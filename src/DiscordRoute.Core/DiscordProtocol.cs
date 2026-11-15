using System.Net.WebSockets;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;

namespace DiscordRoute.Core;

public static class VoiceEndpoint
{
    public static Uri Parse(string text)
    {
        text = text.Trim();
        if (!text.Contains("://", StringComparison.Ordinal)) text = "wss://" + text;
        if (!Uri.TryCreate(text, UriKind.Absolute, out var uri) || uri.Scheme != "wss" ||
            uri.UserInfo.Length != 0 || uri.Fragment.Length != 0 ||
            !Regex.IsMatch(uri.Host, @"^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.discord\.media$", RegexOptions.IgnoreCase) ||
            uri.AbsolutePath != "/" || uri.Port is < 1 or > 65535)
            throw new InvalidDataException("Нужен адрес голосового сервера вида имя.discord.media:порт.");
        return new UriBuilder(uri) { Query = "v=8" }.Uri;
    }

    public static IEnumerable<Uri> Extract(string text)
    {
        foreach (Match match in Regex.Matches(text, @"(?<![\w.-])[a-z0-9][a-z0-9-]{0,62}\.discord\.media(?::[0-9]{1,5})?(?![\w.:-])", RegexOptions.IgnoreCase))
        {
            Uri? endpoint = null;
            try { endpoint = Parse(match.Value); } catch (InvalidDataException) { }
            if (endpoint != null) yield return endpoint;
        }
    }
}

public static class DiscordProtocol
{
    public static bool IsHello(string json, int opcode)
    {
        try
        {
            using var document = JsonDocument.Parse(json);
            var root = document.RootElement;
            return root.ValueKind == JsonValueKind.Object && root.TryGetProperty("op", out var op) &&
                op.ValueKind == JsonValueKind.Number && op.TryGetInt32(out var number) && number == opcode &&
                root.TryGetProperty("d", out var data) && data.ValueKind == JsonValueKind.Object &&
                data.TryGetProperty("heartbeat_interval", out var interval) && interval.ValueKind == JsonValueKind.Number &&
                interval.TryGetDouble(out var duration) && duration > 0 && duration <= 300_000;
        }
        catch (JsonException) { return false; }
    }

    public static async Task<bool> ReadHelloAsync(Uri endpoint, int opcode, CancellationToken cancellationToken)
    {
        using var socket = new ClientWebSocket();
        socket.Options.Proxy = null;
        await socket.ConnectAsync(endpoint, cancellationToken);
        var buffer = new byte[16_384];
        var used = 0;
        ValueWebSocketReceiveResult result;
        do
        {
            result = await socket.ReceiveAsync(buffer.AsMemory(used), cancellationToken);
            if (result.MessageType != WebSocketMessageType.Text) return false;
            used += result.Count;
            if (used == buffer.Length && !result.EndOfMessage) return false;
        } while (!result.EndOfMessage);
        return IsHello(Encoding.UTF8.GetString(buffer, 0, used), opcode);
    }
}
