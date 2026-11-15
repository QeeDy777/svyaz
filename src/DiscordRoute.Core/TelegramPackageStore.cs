using System.Buffers.Binary;
using System.Text.Json;
using System.Text.RegularExpressions;

namespace DiscordRoute.Core;

public sealed record TelegramRelease(string Version, Uri Url, string Digest, long Size);

public sealed class TelegramPackageStore(string directory, Action<string> log)
{
    public const string Executable = "TgWsProxy_windows.exe";
    public const string LatestUrl = "https://api.github.com/repos/Flowseal/tg-ws-proxy/releases/latest";
    private const int MaximumBytes = 128 * 1024 * 1024;

    public Package? Open(string? id)
    {
        if (id == null) return null;
        if (!Regex.IsMatch(id, @"\A[a-f0-9]{32}\z")) throw new InvalidDataException("Повреждён идентификатор компонента Telegram.");
        var root = Path.Combine(directory, id);
        var metadata = ReadMetadata(root);
        return new Package(id, metadata.Version, root);
    }

    private static TelegramRelease ReadMetadata(string root)
    {
        var path = Path.Combine(root, "release.json");
        if (new FileInfo(path).Length > 65536) throw new InvalidDataException("Повреждены сведения о компоненте Telegram.");
        return JsonSerializer.Deserialize<TelegramRelease>(File.ReadAllText(path))
            ?? throw new InvalidDataException("Нет сведений о компоненте Telegram.");
    }

    public static TelegramRelease ParseRelease(string json)
    {
        using var document = JsonDocument.Parse(json);
        var release = document.RootElement;
        if (release.GetProperty("draft").GetBoolean() || release.GetProperty("prerelease").GetBoolean())
            throw new InvalidDataException("Ожидался стабильный релиз Telegram-компонента.");
        var version = release.GetProperty("tag_name").GetString();
        if (string.IsNullOrWhiteSpace(version) || version.Length > 100) throw new InvalidDataException("Неверная версия Telegram-компонента.");
        var assets = release.GetProperty("assets").EnumerateArray().Where(a => a.GetProperty("name").GetString() == Executable).ToArray();
        if (assets.Length != 1) throw new InvalidDataException("Нет однозначного Windows x64 EXE tg-ws-proxy.");
        var asset = assets[0];
        var size = asset.GetProperty("size").GetInt64();
        var digest = asset.TryGetProperty("digest", out var hash) ? hash.GetString() : null;
        if (size is < 256 or > MaximumBytes || digest == null || !Regex.IsMatch(digest, @"\Asha256:[0-9a-fA-F]{64}\z"))
            throw new InvalidDataException("Нет допустимого размера или SHA-256 tg-ws-proxy.");
        if (!Uri.TryCreate(asset.GetProperty("browser_download_url").GetString(), UriKind.Absolute, out var url) ||
            url.Scheme != "https" || url.Host != "github.com" || !url.IsDefaultPort || url.UserInfo.Length != 0 ||
            !url.AbsolutePath.StartsWith("/Flowseal/tg-ws-proxy/releases/download/", StringComparison.Ordinal) ||
            !url.AbsolutePath.EndsWith("/" + Executable, StringComparison.Ordinal) || url.Query.Length != 0 || url.Fragment.Length != 0)
            throw new InvalidDataException("Неожиданный источник tg-ws-proxy.");
        return new(version, url, digest, size);
    }

    public async Task<Package?> StageLatestAsync(string? currentVersion, CancellationToken ct)
    {
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct);
        timeout.CancelAfter(TimeSpan.FromMinutes(3));
        try
        {
            using var response = await PackageStore.GetAsync(new Uri(LatestUrl), timeout.Token);
            var json = await PackageStore.ReadLimitedAsync(response, 2 * 1024 * 1024, timeout.Token);
            var release = ParseRelease(System.Text.Encoding.UTF8.GetString(json));
            if (release.Version == currentVersion) return null;
            log("Telegram: скачиваем официальный tg-ws-proxy " + release.Version);
            using var download = await PackageStore.GetAsync(release.Url, timeout.Token);
            var bytes = await PackageStore.ReadLimitedAsync(download, MaximumBytes, timeout.Token);
            timeout.Token.ThrowIfCancellationRequested();
            return Install(release, bytes);
        }
        catch (OperationCanceledException) when (!ct.IsCancellationRequested)
        { throw new IOException("Загрузка Telegram-компонента превысила 3 минуты."); }
    }

    public Package Install(TelegramRelease release, byte[] bytes)
    {
        VerifyExecutable(bytes, release);
        var id = Guid.NewGuid().ToString("N");
        var root = Path.Combine(directory, id);
        Directory.CreateDirectory(root);
        try
        {
            File.WriteAllBytes(Path.Combine(root, Executable), bytes);
            File.WriteAllText(Path.Combine(root, "release.json"), JsonSerializer.Serialize(release));
            return new Package(id, release.Version, root);
        }
        catch { Directory.Delete(root, true); throw; }
    }

    public static void Verify(Package package)
    {
        var path = Path.Combine(package.Root, Executable);
        if (new FileInfo(path).Length > MaximumBytes) throw new InvalidDataException("Telegram-компонент превышает лимит размера.");
        VerifyExecutable(File.ReadAllBytes(path), ReadMetadata(package.Root));
    }

    private static void VerifyExecutable(byte[] bytes, TelegramRelease release)
    {
        if (bytes.LongLength != release.Size || bytes.Length is < 256 or > MaximumBytes)
            throw new InvalidDataException("Неверный размер Telegram-компонента.");
        PackageStore.VerifyDigest(bytes, release.Digest);
        var pe = BinaryPrimitives.ReadInt32LittleEndian(bytes.AsSpan(60, 4));
        if (bytes[0] != 'M' || bytes[1] != 'Z' || pe < 64 || pe > bytes.Length - 24 ||
            !bytes.AsSpan(pe, 4).SequenceEqual("PE\0\0"u8) || BinaryPrimitives.ReadUInt16LittleEndian(bytes.AsSpan(pe + 4, 2)) != 0x8664)
            throw new InvalidDataException("Telegram-компонент не является Windows x64 EXE.");
    }
}
