using System.Net.WebSockets;
using System.Text;
using DiscordRoute.Core;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Http;
using Microsoft.Extensions.Logging;

static class LocalProtocolChecks
{
    public static async Task RunAsync(Action<bool, string> check)
    {
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
        using var timeout = new CancellationTokenSource(TimeSpan.FromMilliseconds(150));
        try { await DiscordProtocol.ReadHelloAsync(new Uri(baseUrl + "/wait"), 8, timeout.Token); throw new Exception("Expected timeout"); }
        catch (OperationCanceledException) { check(true, "Real WebSocket cancellation"); }
        await app.StopAsync();
    }
}
