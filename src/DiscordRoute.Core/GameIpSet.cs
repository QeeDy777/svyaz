using System.Globalization;
using System.Net;
using System.Net.Sockets;
using System.Text;

namespace DiscordRoute.Core;

public sealed class GameIpSet(string path, Action<string> log, Func<CancellationToken, Task<string>>? download = null)
{
    public const string Source = "https://raw.githubusercontent.com/Flowseal/zapret-discord-youtube/refs/heads/main/.service/ipset-service.txt";
    private const int MaximumBytes = 2 * 1024 * 1024;
    public string FilePath => path;

    public async Task EnsureAsync(CancellationToken ct)
    {
        string? cached = null;
        try
        {
            if (File.Exists(path) && new FileInfo(path).Length <= MaximumBytes)
                cached = Validate(await File.ReadAllTextAsync(path, ct));
        }
        catch (InvalidDataException ex) { log("Кэш IPSet отклонён: " + ex.Message); }
        if (cached != null && File.GetLastWriteTimeUtc(path) > DateTime.UtcNow.AddHours(-6)) return;
        try
        {
            log("Загружаем IPSet для Game Filter из репозитория Flowseal…");
            var content = Validate(await (download ?? DownloadAsync)(ct));
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            var temporary = path + ".tmp";
            try
            {
                await File.WriteAllTextAsync(temporary, content, new UTF8Encoding(false), ct);
                File.Move(temporary, path, true);
            }
            finally { if (File.Exists(temporary)) File.Delete(temporary); }
            log("IPSet загружен. Game Filter будет использовать сети из этого списка.");
        }
        catch (Exception ex) when (!ct.IsCancellationRequested && ex is HttpRequestException or IOException or InvalidDataException or OperationCanceledException)
        {
            if (cached == null) throw new IOException("Game Filter не запущен: не удалось загрузить рабочий IPSet. " + ex.Message, ex);
            log("IPSet не обновлён; используем сохранённый список: " + ex.Message);
        }
    }

    private static async Task<string> DownloadAsync(CancellationToken ct)
    {
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct);
        timeout.CancelAfter(TimeSpan.FromSeconds(20));
        using var client = new HttpClient(new HttpClientHandler { AllowAutoRedirect = false, UseProxy = false });
        using var response = await client.GetAsync(Source, HttpCompletionOption.ResponseHeadersRead, timeout.Token);
        response.EnsureSuccessStatusCode();
        await response.Content.LoadIntoBufferAsync(MaximumBytes, timeout.Token);
        return await response.Content.ReadAsStringAsync(timeout.Token);
    }

    public static string Validate(string text)
    {
        if (text.Length > MaximumBytes) throw new InvalidDataException("Список IPSet слишком большой.");
        var result = new StringBuilder();
        var count = 0;
        foreach (var raw in text.TrimStart('\uFEFF').Split('\n'))
        {
            var line = raw.Trim();
            if (line.Length == 0 || line.StartsWith('#')) continue;
            var pair = line.Split('/');
            if (line.Length > 80 || pair.Length != 2 || pair[0].Contains('%') || !IPAddress.TryParse(pair[0], out var address) ||
                !int.TryParse(pair[1], NumberStyles.None, CultureInfo.InvariantCulture, out var prefix))
                throw new InvalidDataException("IPSet содержит строку, которая не является IP-сетью.");
            var ipv4 = address.AddressFamily == AddressFamily.InterNetwork;
            if ((ipv4 && pair[0].Split('.').Length != 4) || prefix < 1 || prefix > (ipv4 ? 32 : 128))
                throw new InvalidDataException("IPSet содержит недопустимую сеть или маску.");
            if (line == "203.0.113.113/32") continue;
            if (++count > 100_000) throw new InvalidDataException("Слишком много сетей в IPSet.");
            result.Append(address).Append('/').Append(prefix).Append('\n');
        }
        if (count == 0) throw new InvalidDataException("IPSet пуст или содержит только адрес-заглушку.");
        return result.ToString();
    }
}
