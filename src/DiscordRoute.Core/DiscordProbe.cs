using System.Diagnostics;
using System.Net.Http.Headers;
using System.Text.Json;

namespace DiscordRoute.Core;

public sealed record CheckResult(bool Success, string Detail);
public sealed record ProbeResult(CheckResult Api, CheckResult Gateway, CheckResult Voice, YouTubeResult YouTube)
{
    public bool Success => Api.Success && Gateway.Success && Voice.Success && YouTube.Success;
}

public sealed class DiscordProbe(Uri voiceEndpoint)
{
    public async Task<ProbeResult> RunAsync(CancellationToken ct)
    {
        var voice = MeasureAsync(async token => await DiscordProtocol.ReadHelloAsync(voiceEndpoint, 8, token), ct);
        var control = RunControlAsync(ct);
        var youtube = YouTubeProbe.RunAsync(ct);
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
                ? new(true, $"OK · {watch.ElapsedMilliseconds} мс")
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
    public async Task<Strategy?> SelectAsync(Package package, IEnumerable<Strategy> strategies, string? preferred, CancellationToken ct)
    {
        var keepRunning = false;
        try
        {
            foreach (var strategy in strategies.OrderBy(s => s.Name == preferred ? 0 : 1))
            {
                ct.ThrowIfCancellationRequested();
                await runner.StopAsync();
                report(strategy, null, "Запуск");
                try { await runner.StartAsync(package, strategy, ct); }
                catch (Exception ex) when (ex is IOException or InvalidOperationException or System.ComponentModel.Win32Exception)
                { report(strategy, null, "Ошибка запуска: " + ex.Message); continue; }
                var first = await probe(ct);
                report(strategy, first, first.Success ? "Повторная проверка" : "Не прошла");
                if (!first.Success || !runner.IsRunning) continue;
                var second = await probe(ct);
                report(strategy, second, second.Success ? "Подходит" : "Нестабильна");
                if (!second.Success || !runner.IsRunning) continue;
                keepRunning = true;
                return strategy;
            }
            return null;
        }
        finally { if (!keepRunning) await runner.StopAsync(); }
    }
}
