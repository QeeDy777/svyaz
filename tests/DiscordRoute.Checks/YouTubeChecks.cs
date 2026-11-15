using System.Net;
using DiscordRoute.Core;

internal static class YouTubeChecks
{
    public static async Task RunAsync(Action<bool, string> check, bool live)
    {
        var requests = new List<string>();
        using (var client = new HttpClient(new Handler((request, _) =>
        {
            requests.Add(request.RequestUri!.AbsoluteUri);
            check(request.Method == HttpMethod.Get && request.Headers.CacheControl?.NoCache == true, "YouTube probe is an uncached GET");
            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.NoContent));
        })))
        {
            var result = await YouTubeProbe.RunAsync(client, default);
            check(result.Success && requests.SequenceEqual(["https://www.youtube.com/generate_204", "https://redirector.googlevideo.com/generate_204"]),
                "YouTube requires both fixed HTTPS control endpoints");
        }
        foreach (var status in new[] { HttpStatusCode.OK, HttpStatusCode.Redirect, HttpStatusCode.Forbidden, HttpStatusCode.TooManyRequests })
        {
            using var client = new HttpClient(new Handler((request, _) => Task.FromResult(new HttpResponseMessage(
                request.RequestUri!.Host == "www.youtube.com" ? HttpStatusCode.NoContent : status)
                { Content = new StringContent("<html>blocked</html>") })));
            var result = await YouTubeProbe.RunAsync(client, default);
            check(result.Site.Success && !result.GoogleVideo.Success && !result.Success, "YouTube rejects unexpected Googlevideo HTTP " + (int)status);
        }
        using (var client = new HttpClient(new Handler((_, _) => throw new HttpRequestException("TLS failure"))))
            check(!(await YouTubeProbe.RunAsync(client, default)).Success, "YouTube handles network or TLS failure as a failed probe");
        using (var client = new HttpClient(new Handler(async (_, token) =>
        {
            await Task.Delay(Timeout.InfiniteTimeSpan, token);
            return new(HttpStatusCode.NoContent);
        })))
        using (var cancel = new CancellationTokenSource(TimeSpan.FromMilliseconds(100)))
        {
            try { await YouTubeProbe.RunAsync(client, cancel.Token); throw new Exception("YouTube cancellation ignored"); }
            catch (OperationCanceledException) { check(true, "YouTube propagates caller cancellation instead of a negative result"); }
        }
        if (live)
        {
            using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(20));
            var result = await YouTubeProbe.RunAsync(deadline.Token);
            Console.WriteLine("Live YouTube HTTPS: " + result.Detail);
            check(result.Success, "Live YouTube and Googlevideo endpoints return the expected response");
        }
    }

    private sealed class Handler(Func<HttpRequestMessage, CancellationToken, Task<HttpResponseMessage>> send) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken) => send(request, cancellationToken);
    }
}
