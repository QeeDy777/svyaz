using System.Text;
using System.Security.Cryptography;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal sealed class VoiceDiscovery
{
    internal sealed record Observation(Uri Endpoint, string Fingerprint, DateTimeOffset ObservedAt);
    private (string Path, DateTime Modified, long Length)[] previousFiles = [];
    private IReadOnlyList<Observation> previousResult = [];

    public IReadOnlyList<Observation> Find(string roaming)
    {
        var files = new List<FileInfo>();
        foreach (var app in new[] { "discord", "discordptb", "discordcanary" })
        {
            var logs = Path.Combine(roaming, app, "logs");
            if (!Directory.Exists(logs)) continue;
            try
            {
                files.AddRange(new DirectoryInfo(logs).EnumerateFiles("*", new EnumerationOptions
                { RecurseSubdirectories = true, MaxRecursionDepth = 2, IgnoreInaccessible = true, AttributesToSkip = FileAttributes.ReparsePoint })
                    .Where(f => (f.Name.Contains(".log", StringComparison.OrdinalIgnoreCase) || f.Extension == ".txt") &&
                        f.LastWriteTimeUtc > DateTime.UtcNow.AddDays(-7)));
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException) { }
        }
        var recent = files.OrderByDescending(f => f.LastWriteTimeUtc).Take(12)
            .Select(f => (Path: f.FullName, Modified: f.LastWriteTimeUtc, f.Length)).ToArray();
        if (recent.SequenceEqual(previousFiles)) return previousResult;
        var result = new List<Observation>(3);
        var seen = new HashSet<Uri>();
        var readFailed = false;
        foreach (var file in recent)
        {
            try
            {
                using var stream = new FileStream(file.Path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
                stream.Seek(Math.Max(0, stream.Length - 2 * 1024 * 1024), SeekOrigin.Begin);
                using var reader = new StreamReader(stream, Encoding.UTF8);
                foreach (var line in reader.ReadToEnd().Split('\n').Reverse())
                {
                    if (line.Contains("(stream)", StringComparison.Ordinal)) continue;
                    foreach (var endpoint in VoiceEndpoint.Extract(line))
                    {
                        if (!seen.Add(endpoint)) continue;
                        result.Add(new(endpoint, Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(line))), file.Modified));
                        if (result.Count == 3) break;
                    }
                    if (result.Count == 3) break;
                }
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException) { readFailed = true; }
            if (result.Count == 3) break;
        }
        if (!readFailed)
        {
            previousFiles = recent;
            previousResult = result;
        }
        return result;
    }
}
