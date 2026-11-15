using System.Net;

namespace DiscordRoute.Core;

public sealed record YouTubeResult(CheckResult Site, CheckResult GoogleVideo)
{
    public bool Success => Site.Success && GoogleVideo.Success;
    public string Detail => "Сайт: " + Site.Detail + "; Googlevideo: " + GoogleVideo.Detail;
}

public static class YouTubeProbe
{
    public static async Task<YouTubeResult> RunAsync(CancellationToken ct)
    {
        using var client = new HttpClient(new HttpClientHandler
        { UseProxy = false, AllowAutoRedirect = false, UseCookies = false, MaxResponseHeadersLength = 16 });
        return await RunAsync(client, ct);
    }

    internal static async Task<YouTubeResult> RunAsync(HttpClient client, CancellationToken ct)
    {
        // The redirector confirms HTTPS connectivity; it does not prove video playback.
        var site = CheckAsync(client, "https://www.youtube.com/generate_204", ct);
        var video = CheckAsync(client, "https://redirector.googlevideo.com/generate_204", ct);
        await Task.WhenAll(site, video);
        return new(await site, await video);
    }

    private static Task<CheckResult> CheckAsync(HttpClient client, string address, CancellationToken ct) =>
        DiscordProbe.MeasureAsync(async token =>
        {
            using var request = new HttpRequestMessage(HttpMethod.Get, address);
            request.Headers.CacheControl = new() { NoCache = true, NoStore = true };
            using var response = await client.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, token);
            if (response.StatusCode != HttpStatusCode.NoContent)
                throw new HttpRequestException("Ожидался HTTP 204, получен " + (int)response.StatusCode);
            return true;
        }, ct);
}
