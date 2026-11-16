using DiscordRoute.Core;

internal static class QualityChecks
{
    public static void Run(Action<bool, string> check)
    {
        CheckResult Timed(long ms) => new(true, "OK", ElapsedMs: ms);
        var stable = ResponseQuality.From(new[] { Timed(100), Timed(110), Timed(105) });
        var variable = ResponseQuality.From(new[] { Timed(20), Timed(30), Timed(400) });
        check(stable.MedianMs == 105 && stable.SpreadMs == 10 && stable.Score < variable.Score, "Stable responses outrank fast but variable replies");
        check(!ResponseQuality.From(new[] { Timed(10), new CheckResult(true, "no timing"), Timed(10) }).CanRank, "All ranking samples require numeric timing");
        check(!ResponseQuality.From(new[] { Timed(10), new CheckResult(false, "timeout"), Timed(10) }).CanRank, "Failed sample excludes candidate from quality ranking");
        check(ResponseQuality.From(new[] { Timed(10), new CheckResult(false, "skipped", true) }).Total == 1, "Unselected checks never count against quality");
        check(ResponseQuality.From(new[] { Timed(10) }).SpreadMs == null, "Single reply does not invent stability");
        check(ResponseQuality.From(Array.Empty<CheckResult>()).MedianMs == null, "Empty sample does not invent latency");
        var history = new ResponseHistory();
        foreach (var ms in new long[] { 999, 100, 110, 120, 130, 140 }) history.Add(Timed(ms));
        check(history.Quality.Total == 5 && history.Quality.SpreadMs == 40, "Quality window retains only five latest samples");
        history.Add(new(false, "timeout"));
        check(history.Quality.Successful == 4 && history.Quality.Total == 5, "Recent failure remains in success ratio");
        history.Clear();
        check(history.Quality.Total == 0, "Reset discards timings from previous connection");
        var skipped = new CheckResult(false, "skipped", true);
        var sample = ResponseQuality.Sample(new(skipped, skipped, skipped, new(Timed(20), Timed(80))));
        check(sample.Success && sample.ElapsedMs == 80, "Pass latency uses the slowest selected service only");
        check(ResponseQuality.Sample(new(Timed(1), Timed(2), new(true, "legacy"), new(Timed(3), Timed(4)))).ElapsedMs == null, "Missing numeric timing is not parsed or invented");
    }

    public static async Task RunAsync(Action<bool, string> check)
    {
        ProbeResult Timed(long ms) { var c = new CheckResult(true, "OK", ElapsedMs: ms); return new(c, c, c, new(c, c)); }
        var failure = Timed(1) with { Voice = new(false, "timeout") };
        var package = new Package("fixture", "1", "unused");
        var strategies = new[] { new Strategy("fast-variable", "unused"), new Strategy("steady", "unused"), new Strategy("slow", "unused") };
        var runner = new FakeRunner();
        var count = 0;
        var engine = new SelectionEngine(runner, _ => Task.FromResult(runner.Started[^1] switch
        {
            "fast-variable" => Timed(new long[] { 20, 30, 400 }[count++ % 3]),
            "steady" => Timed(100), _ => Timed(200)
        }), (_, _, _) => { });
        var chosen = await engine.SelectAsync(package, strategies, null, default, true);
        check(chosen?.Name == "steady" && runner.Started.SequenceEqual(new[] { "fast-variable", "steady", "slow", "steady" }), "Quality selector compares candidates then restarts steady winner");
        runner = new(); count = 0;
        engine = new(runner, _ => Task.FromResult(runner.Started[^1] == "fast-variable" && runner.Started.Count > 3
            ? failure : Timed(runner.Started[^1] == "fast-variable" ? 10 : 100)), (_, _, _) => { });
        chosen = await engine.SelectAsync(package, strategies, null, default, true);
        check(chosen?.Name == "steady" && runner.Started.Count == 5, "Winner must pass again; failed confirmation tries next ranked candidate");
        runner = new();
        engine = new(runner, _ => Task.FromResult(runner.Started[^1] == "0" ? Timed(100) : failure), (_, _, _) => { });
        chosen = await engine.SelectAsync(package, Enumerable.Range(0, 22).Select(n => new Strategy(n.ToString(), "unused")), null, default, true);
        check(chosen?.Name == "0" && runner.Started.Count == 8, "After first candidate only six further strategies are attempted");
        var all = Enumerable.Range(0, 22).Select(n => new Strategy(n.ToString(), "unused")).ToArray();
        runner = new();
        var reports = new List<string>();
        engine = new(runner, _ => Task.FromResult(Timed(100 - int.Parse(runner.Started[^1]))), (_, _, outcome) => reports.Add(outcome));
        chosen = await engine.SelectAsync(package, all, null, default, fullScan: true);
        check(chosen?.Name == "21" && runner.Started.Take(22).SequenceEqual(all.Select(s => s.Name)) && runner.Started.Count == 23,
            "Full scan checks all 22 working strategies and confirms the best even beyond the quick limit");
        check(reports.Contains("Запуск · 22/22") && reports.Contains("Запуск · подтверждение"), "Full scan reports candidate count and final confirmation");
        runner = new();
        engine = new(runner, _ => Task.FromResult(runner.Started[^1] is "0" or "21" ? Timed(runner.Started[^1] == "21" ? 10 : 100) : failure), (_, _, _) => { });
        chosen = await engine.SelectAsync(package, all, null, default, fullScan: true);
        check(chosen?.Name == "21" && runner.Started.Count == 23, "Full scan continues past six failed alternatives to a late winner");
        runner = new();
        engine = new(runner, _ => Task.FromResult(runner.Started.Count > 22 && runner.Started[^1] == "21" ? failure : Timed(100 - int.Parse(runner.Started[^1]))), (_, _, _) => { });
        chosen = await engine.SelectAsync(package, all, null, default, fullScan: true);
        check(chosen?.Name == "20" && runner.Started.Count == 24, "Full scan rejects failed winner confirmation and validates the next ranked candidate");
        runner = new(); count = 0;
        using (var fullCancel = new CancellationTokenSource())
        {
            engine = new(runner, _ => { if (++count == 25) fullCancel.Cancel(); return Task.FromResult(Timed(10)); }, (_, _, _) => { });
            try { await engine.SelectAsync(package, all, null, fullCancel.Token, fullScan: true); throw new Exception("Cancelled full scan accepted"); }
            catch (OperationCanceledException) { check(!runner.IsRunning, "Cancelling a full scan beyond the quick limit stops its process"); }
        }
        runner = new(); count = 0;
        using var cancel = new CancellationTokenSource();
        engine = new(runner, _ => { if (++count == 4) cancel.Cancel(); return Task.FromResult(Timed(10)); }, (_, _, _) => { });
        try { await engine.SelectAsync(package, strategies, null, cancel.Token, true); throw new Exception("Cancelled comparison accepted"); }
        catch (OperationCanceledException) { check(!runner.IsRunning, "Cancelling quality comparison leaves no process running"); }
        runner = new(); count = 0;
        engine = new(runner, _ => { count++; return Task.FromResult(Timed(10)); }, (_, _, _) => { });
        chosen = await engine.SelectAsync(package, strategies.Take(1), null, default);
        check(chosen == strategies[0] && runner.Started.Count == 1 && count == 2, "Manual selection retains two checks without switching candidates");
    }
}
