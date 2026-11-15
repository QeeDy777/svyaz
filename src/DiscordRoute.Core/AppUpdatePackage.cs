using System.IO.Compression;
using System.Text.Json;
using System.Text.RegularExpressions;

namespace DiscordRoute.Core;

public sealed record AppRelease(Version Version, Uri Url, string Digest, long Size);

public static class AppUpdatePackage
{
    public const string LatestUrl = "https://api.github.com/repos/QeeDy777/svyaz/releases/latest";
    public static readonly string[] RuntimeFiles = ["Связь.exe", "D3DCompiler_47_cor3.dll", "PenImc_cor3.dll",
        "PresentationNative_cor3.dll", "vcruntime140_cor3.dll", "wpfgfx_cor3.dll"];
    private static readonly HashSet<string> Allowed = new(RuntimeFiles.Concat(new[] { "README.txt", "VALIDATION.txt", "SIGNING.txt",
        "DOTNET-LICENSE.txt", "DOTNET-THIRD-PARTY-NOTICES.txt", "WINDOWS-DESKTOP-LICENSE.txt", "TG-WS-PROXY-LICENSE.txt" }), StringComparer.Ordinal);
    private const int DownloadLimit = 160 * 1024 * 1024;

    public static Version ParseVersion(string value)
    {
        if (string.IsNullOrEmpty(value) || value.Length > 32 || !Regex.IsMatch(value, @"\A(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\z") ||
            !Version.TryParse(value, out var version)) throw new InvalidDataException("Неверная версия «Связи».");
        return version;
    }

    public static AppRelease ParseRelease(string json)
    {
        using var document = JsonDocument.Parse(json);
        var root = document.RootElement;
        if (root.GetProperty("draft").GetBoolean() || root.GetProperty("prerelease").GetBoolean())
            throw new InvalidDataException("Обновление должно быть стабильным релизом.");
        var tag = root.GetProperty("tag_name").GetString() ?? "";
        if (!tag.StartsWith('v')) throw new InvalidDataException("Неверный тег «Связи».");
        var version = ParseVersion(tag[1..]);
        var assets = root.GetProperty("assets").EnumerateArray().ToArray();
        var signed = $"Svyaz-{version}-win-x64-signed.zip";
        var unsigned = $"Svyaz-{version}-win-x64-unsigned.zip";
        var matches = assets.Where(a => a.GetProperty("name").GetString() == signed).ToArray();
        if (matches.Length == 0) matches = assets.Where(a => a.GetProperty("name").GetString() == unsigned).ToArray();
        if (matches.Length != 1) throw new InvalidDataException("Нет однозначного Windows x64 архива «Связи».");
        var asset = matches[0];
        var digest = asset.GetProperty("digest").GetString() ?? "";
        var size = asset.GetProperty("size").GetInt64();
        if (size is < 1 or > DownloadLimit || !Regex.IsMatch(digest, @"\Asha256:[0-9a-fA-F]{64}\z"))
            throw new InvalidDataException("Нет допустимого размера или SHA-256 обновления.");
        var expected = $"https://github.com/QeeDy777/svyaz/releases/download/{tag}/{asset.GetProperty("name").GetString()}";
        if (asset.GetProperty("browser_download_url").GetString() != expected)
            throw new InvalidDataException("Неожиданный источник обновления «Связи».");
        return new(version, new Uri(expected), digest, size);
    }

    public static async Task<(AppRelease Release, byte[] Bytes)?> DownloadAsync(Version current, CancellationToken ct)
    {
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct);
        timeout.CancelAfter(TimeSpan.FromMinutes(3));
        try
        {
            using var response = await PackageStore.GetAsync(new Uri(LatestUrl), timeout.Token);
            var release = ParseRelease(System.Text.Encoding.UTF8.GetString(await PackageStore.ReadLimitedAsync(response, 2 * 1024 * 1024, timeout.Token)));
            if (release.Version <= current) return null;
            using var download = await PackageStore.GetAsync(release.Url, timeout.Token);
            var bytes = await PackageStore.ReadLimitedAsync(download, DownloadLimit, timeout.Token);
            timeout.Token.ThrowIfCancellationRequested();
            return (release, bytes);
        }
        catch (OperationCanceledException) when (!ct.IsCancellationRequested)
        { throw new IOException("Загрузка «Связи» превысила 3 минуты."); }
    }

    // The caller owns a new empty staging directory; existing installations are never extracted over.
    public static void Extract(AppRelease release, byte[] bytes, string destination, CancellationToken ct = default)
    {
        if (bytes.LongLength != release.Size || bytes.Length > DownloadLimit) throw new InvalidDataException("Неверный размер обновления.");
        PackageStore.VerifyDigest(bytes, release.Digest);
        using var archive = new ZipArchive(new MemoryStream(bytes));
        var names = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        long total = 0;
        foreach (var entry in archive.Entries)
        {
            if (!Allowed.Contains(entry.FullName) || !names.Add(entry.FullName) || ((entry.ExternalAttributes >> 16) & 0xF000) == 0xA000 ||
                entry.Length <= 0 || entry.Length > DownloadLimit || (total += entry.Length) > 256L * 1024 * 1024)
                throw new InvalidDataException("Недопустимое содержимое обновления.");
        }
        if (RuntimeFiles.Any(f => !names.Contains(f))) throw new InvalidDataException("В обновлении отсутствуют файлы WPF.");
        if (Directory.Exists(destination) && Directory.EnumerateFileSystemEntries(destination).Any())
            throw new IOException("Каталог распаковки должен быть пустым.");
        Directory.CreateDirectory(destination);
        foreach (var entry in archive.Entries)
        {
            ct.ThrowIfCancellationRequested();
            using var input = entry.Open();
            using var output = new FileStream(Path.Combine(destination, entry.FullName), FileMode.CreateNew);
            var buffer = new byte[81920];
            long copied = 0;
            int count;
            while ((count = input.Read(buffer)) != 0)
            {
                ct.ThrowIfCancellationRequested();
                copied += count;
                if (copied > entry.Length) throw new InvalidDataException("Неверный размер файла ZIP.");
                output.Write(buffer, 0, count);
            }
            if (copied != entry.Length) throw new InvalidDataException("Неполный файл обновления.");
        }
    }
}
