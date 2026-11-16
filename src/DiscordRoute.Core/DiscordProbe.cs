using System.Diagnostics;
using System.Net.Http.Headers;
using System.Text.Json;

namespace DiscordRoute.Core;

public sealed record CheckResult(bool Success, string Detail, bool Skipped = false, long? ElapsedMs = null);
public sealed record ProbeResult(CheckResult Api, CheckResult Gateway, CheckResult Voice, YouTubeResult YouTube)
{
    public bool Success => new[] { Api, Gateway, Voice, YouTube.Site, YouTube.GoogleVideo }.All(c => c.Success || c.Skipped) &&
        new[] { Api, Gateway, Voice, YouTube.Site, YouTube.GoogleVideo }.Any(c => !c.Skipped);
}

public sealed class DiscordProbe(Uri? voiceEndpoint, ServiceSelection? selected = null)
{
    public async Task<ProbeResult> RunAsync(CancellationToken ct)
    {
        var services = selected ?? new();
        var skipped = new CheckResult(false, "Не выбрано", true);
        var voice = services.Discord ? MeasureAsync(async token => await DiscordProtocol.ReadHelloAsync(voiceEndpoint ?? throw new InvalidDataException("Нет адреса голосового сервера."), 8, token), ct) : Task.FromResult(skipped);
        var control = services.Discord ? RunControlAsync(ct) : Task.FromResult((Api: skipped, Gateway: skipped));
        var youtube = services.YouTube ? YouTubeProbe.RunAsync(ct) : Task.FromResult(new YouTubeResult(skipped, skipped));
        await Task.WhenAll(voice, control, youtube);
        var (api, gateway) = await control;
        return new(api, gateway, await voice, await youtube);
    }

    public static async Task<(CheckResult Api, CheckResult Gateway)> RunControlAsync(CancellationToken ct)
    {
        var api = MeasureAsync(async token =>
        {
            using var client = new HttpClient(new HttpClientHandler { UseProxy = false, AllowAutoRedirect = false });
            client.DefaultRequestHeaders.UserAgent.ParseAdd("DiscordRoute/0.1");
            client.DefaultRequestHeaders.CacheControl = new CacheControlHeaderValue { NoCache = true };
            using var response = await client.GetAsync("https://discord.com/api/v10/gateway", HttpCompletionOption.ResponseHeadersRead, token);
            if (!response.IsSuccessStatusCode) throw new HttpRequestException("HTTP " + (int)response.StatusCode);
            await using var body = await response.Content.ReadAsStreamAsync(token);
            var buffer = new byte[8193];
            var read = await body.ReadAtLeastAsync(buffer, buffer.Length, throwOnEndOfStream: false, cancellationToken: token);
            if (read > 8192) return false;
            using var json = JsonDocument.Parse(buffer.AsMemory(0, read));
            return json.RootElement.ValueKind == JsonValueKind.Object && json.RootElement.TryGetProperty("url", out var url) &&
                url.ValueKind == JsonValueKind.String && url.GetString() == "wss://gateway.discord.gg";
        }, ct);
        var gateway = MeasureAsync(token => DiscordProtocol.ReadHelloAsync(new Uri("wss://gateway.discord.gg/?v=10&encoding=json"), 10, token), ct);
        await Task.WhenAll(api, gateway);
        return (await api, await gateway);
    }

    internal static async Task<CheckResult> MeasureAsync(Func<CancellationToken, Task<bool>> check, CancellationToken ct)
    {
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct);
        timeout.CancelAfter(TimeSpan.FromSeconds(7));
        var watch = Stopwatch.StartNew();
        try
        {
            return await check(timeout.Token)
                ? new(true, $"OK · {watch.ElapsedMilliseconds} мс", ElapsedMs: watch.ElapsedMilliseconds)
                : new(false, "Нет корректного ответа");
        }
        catch (OperationCanceledException) when (!ct.IsCancellationRequested) { return new(false, "Тайм-аут 7 с"); }
        catch (Exception ex) when (ex is HttpRequestException or System.Net.WebSockets.WebSocketException or JsonException or IOException)
        { return new(false, ex.Message); }
    }
}

public interface IStrategyRunner
{
    bool IsRunning { get; }
    Task StartAsync(Package package, Strategy strategy, CancellationToken ct);
    Task StopAsync();
}

public sealed class SelectionEngine(IStrategyRunner runner, Func<CancellationToken, Task<ProbeResult>> probe,
    Action<Strategy, ProbeResult?, string> report)
{
    public async Task<Strategy?> SelectAsync(Package package, IEnumerable<Strategy> strategies, string? preferred, CancellationToken ct,
        bool compareQuality = false)
    {
        var keepRunning = false;
        var candidates = new List<(Strategy Strategy, ResponseQuality Quality)>();
        var additional = 0;
        try
        {
            foreach (var strategy in strategies.OrderBy(s => s.Name == preferred ? 0 : 1))
            {
                if (candidates.Count > 0 && additional++ >= 6) break;
                var samples = await EvaluateAsync(strategy, compareQuality ? 3 : 2);
                if (samples == null) continue;
                var quality = ResponseQuality.From(samples.Select(ResponseQuality.Sample));
                if (!compareQuality)
                {
                    keepRunning = true;
                    return strategy;
                }
                if (!quality.CanRank)
                {
                    report(strategy, null, "Ошибка измерения времени; сравнение недоступно");
                    continue;
                }
                candidates.Add((strategy, quality));
                if (candidates.Count == 3) break;
            }
            foreach (var candidate in candidates.OrderBy(c => c.Quality.Score))
            {
                // Switching winws invalidates earlier success: confirm the final process again.
                var confirmed = await EvaluateAsync(candidate.Strategy, 2, candidate.Quality);
                if (confirmed == null) continue;
                keepRunning = true;
                return candidate.Strategy;
            }
            return null;
        }
        finally { if (!keepRunning) await runner.StopAsync(); }

        async Task<List<ProbeResult>?> EvaluateAsync(Strategy strategy, int count, ResponseQuality? chosenQuality = null)
        {
            ct.ThrowIfCancellationRequested();
            await runner.StopAsync();
            report(strategy, null, "Запуск");
            try { await runner.StartAsync(package, strategy, ct); }
            catch (Exception ex) when (ex is IOException or InvalidOperationException or System.ComponentModel.Win32Exception)
            { report(strategy, null, "Ошибка запуска: " + ex.Message); return null; }
            var samples = new List<ProbeResult>();
            for (var pass = 0; pass < count; pass++)
            {
                ct.ThrowIfCancellationRequested();
                var result = await probe(ct);
                ct.ThrowIfCancellationRequested();
                samples.Add(result);
                report(strategy, result, !result.Success ? "Не прошла" : pass == count - 1 && chosenQuality != null
                    ? "Выбрана по качеству: " + chosenQuality.Summary : $"Проверка {pass + 1}/{count}");
                if (!result.Success || !runner.IsRunning) return null;
            }
            return samples;
        }
    }
}
