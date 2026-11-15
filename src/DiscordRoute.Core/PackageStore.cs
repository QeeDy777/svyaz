using System.IO.Compression;
using System.Net;
using System.Security.Cryptography;
using System.Text.Json;
using System.Text.RegularExpressions;

namespace DiscordRoute.Core;

public sealed record Package(string Id, string Version, string Root);

public sealed class PackageStore(string directory, Action<string> log)
{
    public const string Repository = "https://github.com/Flowseal/zapret-discord-youtube";
    private const long DownloadLimit = 64 * 1024 * 1024;
    private static readonly HttpClient Client = new(new HttpClientHandler { UseProxy = false, AllowAutoRedirect = false })
    { Timeout = TimeSpan.FromMinutes(3) };

    public Package? Open(string? id)
    {
        if (id == null) return null;
        if (!Regex.IsMatch(id, "^[a-f0-9]{32}$")) throw new InvalidDataException("Повреждён идентификатор пакета.");
        var path = Path.Combine(directory, id);
        var metadata = JsonSerializer.Deserialize<Dictionary<string, string>>(File.ReadAllText(Path.Combine(path, "release.json")))!;
        return new Package(id, metadata["version"], FindRoot(path));
    }

    public async Task<Package?> StageLatestAsync(string? currentVersion, string? previousRoot, CancellationToken ct)
    {
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct);
        timeout.CancelAfter(TimeSpan.FromMinutes(3));
        try { return await StageLatestCoreAsync(currentVersion, previousRoot, timeout.Token); }
        catch (OperationCanceledException) when (!ct.IsCancellationRequested)
        { throw new IOException("Превышено время загрузки GitHub (3 минуты)."); }
    }

    private async Task<Package?> StageLatestCoreAsync(string? currentVersion, string? previousRoot, CancellationToken ct)
    {
        Directory.CreateDirectory(directory);
        using var response = await GetAsync(new Uri("https://api.github.com/repos/Flowseal/zapret-discord-youtube/releases/latest"), ct);
        var bytes = await ReadLimitedAsync(response, 2 * 1024 * 1024, ct);
        using var document = JsonDocument.Parse(bytes);
        var release = document.RootElement;
        if (release.GetProperty("draft").GetBoolean() || release.GetProperty("prerelease").GetBoolean())
            throw new InvalidDataException("Ожидался стабильный релиз.");
        var version = release.GetProperty("tag_name").GetString()!;
        if (version == currentVersion) return null;
        var assets = release.GetProperty("assets").EnumerateArray().Where(a =>
            a.GetProperty("name").GetString() is string name && name.StartsWith("zapret-discord-youtube-", StringComparison.Ordinal) &&
            name.EndsWith(".zip", StringComparison.OrdinalIgnoreCase)).ToArray();
        if (assets.Length != 1) throw new InvalidDataException("В релизе не найден однозначный ZIP-пакет.");
        var asset = assets[0];
        if (asset.GetProperty("size").GetInt64() > DownloadLimit) throw new InvalidDataException("Пакет превышает лимит размера.");
        var digest = asset.TryGetProperty("digest", out var hash) ? hash.GetString() : null;
        if (digest == null || !Regex.IsMatch(digest, "^sha256:[a-fA-F0-9]{64}$"))
            throw new InvalidDataException("GitHub не предоставил SHA-256 пакета. Обновление не установлено.");
        var url = new Uri(asset.GetProperty("browser_download_url").GetString()!);
        if (url.Scheme != "https" || url.Host != "github.com" ||
            !url.AbsolutePath.StartsWith("/Flowseal/zapret-discord-youtube/releases/download/", StringComparison.Ordinal))
            throw new InvalidDataException("Неожиданный источник обновления.");
        log("Скачивание официального пакета " + version);
        using var download = await GetAsync(url, ct);
        var zip = await ReadLimitedAsync(download, DownloadLimit, ct);
        VerifyDigest(zip, digest);
        var id = Guid.NewGuid().ToString("N");
        var stage = Path.Combine(directory, id);
        Directory.CreateDirectory(stage);
        try
        {
            Extract(zip, stage);
            var root = FindRoot(stage);
            PrepareLists(root, previousRoot);
            Strategy.Load(root, log);
            File.WriteAllText(Path.Combine(stage, "release.json"), JsonSerializer.Serialize(new { version }));
            log("Пакет проверен: SHA-256, структура и формат стратегий.");
            return new Package(id, version, root);
        }
        catch { Directory.Delete(stage, true); throw; }
    }

    public static void VerifyDigest(byte[] bytes, string expected)
    {
        var actual = "sha256:" + Convert.ToHexString(SHA256.HashData(bytes));
        if (!string.Equals(actual, expected, StringComparison.OrdinalIgnoreCase))
            throw new InvalidDataException("SHA-256 не совпадает. Пакет отклонён.");
    }

    public static void Extract(byte[] bytes, string target)
    {
        using var zip = new ZipArchive(new MemoryStream(bytes), ZipArchiveMode.Read);
        if (zip.Entries.Count > 4096) throw new InvalidDataException("Слишком много файлов в архиве.");
        long total = 0;
        var paths = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var entry in zip.Entries)
        {
            var path = entry.FullName.Replace('\\', '/').TrimEnd('/');
            var segments = path.Split('/');
            if (path.Length == 0 || path.Length > 240 || path.StartsWith('/') || segments.Any(s =>
                s is "" or "." or ".." || s.EndsWith('.') || s.EndsWith(' ') ||
                s.Any(c => c < 32 || ":*?\"<>|".Contains(c)) ||
                Regex.IsMatch(s, @"^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\.|$)", RegexOptions.IgnoreCase)) ||
                ((entry.ExternalAttributes >> 16) & 0xF000) == 0xA000 || !paths.Add(path))
                throw new InvalidDataException("Недопустимый путь в архиве: " + entry.FullName);
            total += entry.Length;
            if (entry.Length > 64 * 1024 * 1024 || total > 256 * 1024 * 1024)
                throw new InvalidDataException("Распакованный архив превышает лимит.");
            var destination = Path.Combine(target, Path.Combine(segments));
            if (entry.FullName.EndsWith('/') || entry.FullName.EndsWith('\\')) Directory.CreateDirectory(destination);
            else
            {
                Directory.CreateDirectory(Path.GetDirectoryName(destination)!);
                using var input = entry.Open();
                using var output = new FileStream(destination, FileMode.CreateNew);
                var buffer = new byte[81920];
                long copied = 0;
                int count;
                while ((count = input.Read(buffer)) > 0)
                {
                    copied += count;
                    if (copied > entry.Length) throw new InvalidDataException("Некорректный размер файла в ZIP.");
                    output.Write(buffer, 0, count);
                }
                if (copied != entry.Length) throw new InvalidDataException("Неполный файл в ZIP.");
            }
        }
    }

    private static string FindRoot(string stage)
    {
        var candidates = new[] { stage }.Concat(Directory.GetDirectories(stage)).Where(p => File.Exists(Path.Combine(p, "bin", "winws.exe"))).ToArray();
        if (candidates.Length != 1) throw new InvalidDataException("Неверная структура пакета zapret.");
        foreach (var name in new[] { "WinDivert.dll", "WinDivert64.sys", "cygwin1.dll" })
            if (!File.Exists(Path.Combine(candidates[0], "bin", name))) throw new InvalidDataException("Нет " + name);
        if (!Directory.Exists(Path.Combine(candidates[0], "lists"))) throw new InvalidDataException("Нет списков доменов.");
        return candidates[0];
    }

    public static void PrepareLists(string root, string? previousRoot)
    {
        var lists = Path.Combine(root, "lists");
        Directory.CreateDirectory(lists);
        if (previousRoot != null)
            foreach (var file in Directory.GetFiles(Path.Combine(previousRoot, "lists"), "*-user.txt"))
                File.Copy(file, Path.Combine(lists, Path.GetFileName(file)), true);
        foreach (var name in new[] { "list-general-user.txt", "list-exclude-user.txt", "ipset-exclude-user.txt" })
        {
            var file = Path.Combine(lists, name);
            if (!File.Exists(file)) File.WriteAllText(file, name.StartsWith("ipset", StringComparison.Ordinal) ? "203.0.113.113/32\n" : "domain.example.abc\n");
        }
    }

    internal static async Task<HttpResponseMessage> GetAsync(Uri uri, CancellationToken ct)
    {
        for (var attempt = 0; attempt < 5; attempt++)
        {
            if (uri.Scheme != "https" || uri.UserInfo.Length != 0 ||
                uri.Host is not ("api.github.com" or "github.com" or "release-assets.githubusercontent.com"))
                throw new InvalidDataException("Недопустимое перенаправление GitHub.");
            using var request = new HttpRequestMessage(HttpMethod.Get, uri);
            request.Headers.UserAgent.ParseAdd("DiscordRoute/0.1");
            var response = await Client.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, ct);
            if ((int)response.StatusCode is >= 300 and < 400 && response.Headers.Location is Uri location)
            {
                uri = location.IsAbsoluteUri ? location : new Uri(uri, location);
                response.Dispose();
                continue;
            }
            if (!response.IsSuccessStatusCode)
            {
                var status = response.StatusCode;
                response.Dispose();
                throw new HttpRequestException(status is HttpStatusCode.Forbidden or HttpStatusCode.TooManyRequests
                    ? "GitHub ограничил запросы. Попробуйте позже; сохранённый пакет остаётся доступен."
                    : "Ошибка загрузки GitHub: " + status);
            }
            return response;
        }
        throw new InvalidDataException("Слишком много перенаправлений GitHub.");
    }

    internal static async Task<byte[]> ReadLimitedAsync(HttpResponseMessage response, long limit, CancellationToken ct)
    {
        if (response.Content.Headers.ContentLength > limit) throw new InvalidDataException("Ответ сервера слишком большой.");
        await using var input = await response.Content.ReadAsStreamAsync(ct);
        using var output = new MemoryStream();
        var buffer = new byte[81920];
        int count;
        while ((count = await input.ReadAsync(buffer, ct)) > 0)
        {
            if (output.Length + count > limit) throw new InvalidDataException("Превышен лимит загрузки.");
            output.Write(buffer, 0, count);
        }
        return output.ToArray();
    }
}
