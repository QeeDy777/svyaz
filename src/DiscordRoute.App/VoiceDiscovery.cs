using System.Text;
using System.Security.Cryptography;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal static class VoiceDiscovery
{
    public static (Uri Endpoint, string Fingerprint, DateTimeOffset ObservedAt)? Find(Action<string> log)
    {
        var roaming = Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData);
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
            catch (IOException) { }
        }
        foreach (var file in files.OrderByDescending(f => f.LastWriteTimeUtc).Take(12))
        {
            try
            {
                using var stream = new FileStream(file.FullName, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
                stream.Seek(Math.Max(0, stream.Length - 2 * 1024 * 1024), SeekOrigin.Begin);
                using var reader = new StreamReader(stream, Encoding.UTF8);
                // shortcut: Discord text logs are not a public API; update discovery if their format changes.
                var candidate = reader.ReadToEnd().Split('\n').Reverse()
                    .Where(line => !line.Contains("(stream)", StringComparison.Ordinal))
                    .Select(line => (Line: line, Endpoint: VoiceEndpoint.Extract(line).FirstOrDefault()))
                    .FirstOrDefault(item => item.Endpoint != null);
                if (candidate.Endpoint == null) continue;
                log("Голосовой сервер определён из журнала Discord: " + candidate.Endpoint.Authority);
                return (candidate.Endpoint, Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(candidate.Line))), file.LastWriteTimeUtc);
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException) { }
        }
        return null;
    }
}
