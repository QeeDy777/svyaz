using DiscordRoute.Core;

internal static class RecoveryChecks
{
    public static void Run(Action<bool, string> check)
    {
        var now = DateTimeOffset.Parse("2026-11-16T00:00:00Z");
        var r = new ConnectionRecovery();
        r.Invalidate(now);
        check(!r.TryBegin(now.AddMinutes(1), true), "Network changes never start a disabled service");
        r.Enable(); r.Invalidate(now); r.Invalidate(now.AddSeconds(2));
        check(!r.TryBegin(now.AddSeconds(11), true) && !r.TryBegin(now.AddSeconds(12), false), "Recovery debounces events and waits for network");
        check(r.TryBegin(now.AddSeconds(12), true) && !r.TryBegin(now.AddSeconds(13), true), "Recovery serializes retries with backoff");
        r.Failed(now.AddSeconds(14));
        check(r.TryBegin(now.AddSeconds(42), true) && r.TryBegin(now.AddSeconds(102), true) && !r.TryBegin(now.AddHours(1), true), "At most three automatic recovery attempts");
        check(!r.ShouldNotify(now.AddSeconds(119), true) && !r.ShouldNotify(now.AddSeconds(121), false), "Short failures and disabled alerts stay silent");
        check(r.ShouldNotify(now.AddSeconds(121), true) && !r.ShouldNotify(now.AddHours(1), true), "Only one alert per prolonged outage");
        r.Invalidate(now.AddHours(2));
        check(!r.TryBegin(now.AddHours(3), true), "Repeated network events do not reset an exhausted retry budget");
        r.Succeeded();
        check(!r.Pending && !r.ShouldNotify(now.AddHours(1), true), "Success clears outage and pending retry");
        r.Invalidate(now); r.Stop();
        check(!r.Wanted && !r.TryBegin(now.AddHours(1), true) && !r.ShouldNotify(now.AddHours(1), true), "Manual stop cancels retries and notifications");
        check(ConnectionHelp.Describe(new IOException(), false).Message.StartsWith("Нет сетевого"), "Offline failure offers concrete guidance");
        check(ConnectionHelp.Describe(new ConnectionFailure("Введите адрес", true), true).OpenSettings, "Actionable voice failure opens settings");
        var ok = new CheckResult(true, "OK");
        check(ConnectionHelp.Describe(new IOException(), true, new(ok, ok, ok, new(ok, new(false, "timeout")))).Message.Contains("YouTube"), "YouTube failure remains visible in guidance");
    }
}
