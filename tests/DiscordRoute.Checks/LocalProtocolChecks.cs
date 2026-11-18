using System.Net.WebSockets;
using System.Text;
using DiscordRoute.Core;
using DiscordRoute.App;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Http;
using Microsoft.Extensions.Logging;

static class LocalProtocolChecks
{
    public static async Task RunAsync(Action<bool, string> check)
    {
        var fixture = Directory.CreateTempSubdirectory("voice-logs-");
        try
        {
            var logs = fixture.CreateSubdirectory(Path.Combine("discord", "logs"));
            var path = Path.Combine(logs.FullName, "main.log");
            File.WriteAllLines(path, ["old.discord.media", "third.discord.media:2096", "second.discord.media:8443",
                "first.discord.media:2053", "first.discord.media:2053", "(stream) ignore.discord.media"]);
            var discovery = new VoiceDiscovery();
            var recent = discovery.Find(fixture.FullName);
            check(recent.Select(item => item.Endpoint.Authority).SequenceEqual(
                ["first.discord.media:2053", "second.discord.media:8443", "third.discord.media:2096"]),
                "Three recent unique voice hosts preserve ports and skip stream records");
            File.AppendAllText(path, "new.discord.media:443");
            check(discovery.Find(fixture.FullName)[0].Endpoint.Host == "new.discord.media", "Changed log refreshes cached hosts");
        }
        finally { fixture.Delete(recursive: true); }
        Console.WriteLine("Starting local WebSocket integration server…");
        var builder = WebApplication.CreateBuilder();
        builder.Logging.ClearProviders();
        builder.WebHost.UseUrls("http://127.0.0.1:0");
        await using var app = builder.Build();
        app.UseWebSockets();
        app.Map("/voice", async context =>
        {
            using var socket = await context.WebSockets.AcceptWebSocketAsync();
            var bytes = Encoding.UTF8.GetBytes("{\"op\":8,\"d\":{\"heartbeat_interval\":41250}}");
            await socket.SendAsync(bytes.AsMemory(0, 10), WebSocketMessageType.Text, false, context.RequestAborted);
            await socket.SendAsync(bytes.AsMemory(10), WebSocketMessageType.Text, true, context.RequestAborted);
        });
        app.Map("/wrong", async context =>
        {
            using var socket = await context.WebSockets.AcceptWebSocketAsync();
            await socket.SendAsync(Encoding.UTF8.GetBytes("{\"op\":2,\"d\":{}}"), WebSocketMessageType.Text, true, context.RequestAborted);
        });
        app.Map("/large", async context =>
        {
            using var socket = await context.WebSockets.AcceptWebSocketAsync();
            await socket.SendAsync(new byte[17_000], WebSocketMessageType.Text, true, context.RequestAborted);
        });
        app.Map("/wait", async context =>
        {
            using var socket = await context.WebSockets.AcceptWebSocketAsync();
            await Task.Delay(700);
        });
        using var startTimeout = new CancellationTokenSource(TimeSpan.FromSeconds(10));
        await app.StartAsync(startTimeout.Token);
        var baseUrl = app.Urls.Single().Replace("http://", "ws://", StringComparison.Ordinal);
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(10));
        check(await DiscordProtocol.ReadHelloAsync(new Uri(baseUrl + "/voice"), 8, deadline.Token), "Real WebSocket fragmented Hello");
        check(!await DiscordProtocol.ReadHelloAsync(new Uri(baseUrl + "/wrong"), 8, deadline.Token), "Real WebSocket wrong opcode rejected");
        check(!await DiscordProtocol.ReadHelloAsync(new Uri(baseUrl + "/large"), 8, deadline.Token), "Real WebSocket oversized payload rejected");
        var oldVoice = new Uri(baseUrl + "/wrong");
        Uri? currentVoice = oldVoice;
        var changingVoice = new DiscordProbe(oldVoice, resolveVoiceEndpoint: () => currentVoice);
        check(!(await changingVoice.RunVoiceAsync(deadline.Token)).Success, "Stale voice endpoint cannot pass the protocol check");
        currentVoice = new Uri(baseUrl + "/voice");
        check((await changingVoice.RunVoiceAsync(deadline.Token)).Success, "Next selection probe uses the newly discovered voice server");
        currentVoice = null;
        check((await changingVoice.RunVoiceAsync(deadline.Token)).Success, "Temporarily unavailable logs retain the last discovered voice endpoint");
        try { await new DiscordProbe(null).RunVoiceAsync(deadline.Token); throw new Exception("Missing voice address accepted"); }
        catch (InvalidDataException) { check(true, "Missing voice address never passes the probe"); }
        var fixedVoice = new DiscordProbe(new Uri(baseUrl + "/voice"));
        check((await fixedVoice.RunVoiceAsync(deadline.Token)).Success, "Explicit voice endpoint still works without a resolver");
        var wrong = new Uri(baseUrl + "/wrong");
        var large = new Uri(baseUrl + "/large");
        var good = new Uri(baseUrl + "/voice");
        var fallback = new DiscordProbe([wrong, wrong, large, good], new(), () => []);
        var passed = await fallback.RunVoiceAsync(deadline.Token);
        check(passed.Success && passed.Detail.Split(" / ").Length == 3, "Third distinct voice endpoint can confirm the probe");
        var first = await new DiscordProbe([good, wrong], new(), () => []).RunVoiceAsync(deadline.Token);
        check(first.Success && !first.Detail.Contains(" / "), "First Hello ends the search");
        var capped = await new DiscordProbe([wrong, large, new Uri(baseUrl + "/missing"), good], new(), () => []).RunVoiceAsync(deadline.Token);
        check(!capped.Success && capped.Detail.Split(" / ").Length == 3, "Fourth endpoint is never tried after three failures");
        using var timeout = new CancellationTokenSource(TimeSpan.FromMilliseconds(150));
        try { await DiscordProtocol.ReadHelloAsync(new Uri(baseUrl + "/wait"), 8, timeout.Token); throw new Exception("Expected timeout"); }
        catch (OperationCanceledException) { check(true, "Real WebSocket cancellation"); }
        await app.StopAsync();
    }
}
