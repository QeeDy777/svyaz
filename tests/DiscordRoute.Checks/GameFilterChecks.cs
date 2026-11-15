using DiscordRoute.Core;

internal static class GameFilterChecks
{
    public static async Task RunAsync(Action<bool, string> check)
    {
        void Reject(Action action, string name)
        {
            try { action(); }
            catch (InvalidDataException) { check(true, name); return; }
            throw new Exception("FAIL (accepted invalid Game Filter input): " + name);
        }
        check(new GameFilterSettings().Ranges() == ("12", "12", "12"), "Game Filter defaults to disabled port placeholders");
        check((new GameFilterSettings { Mode = GameFilterMode.All }).Ranges() == ("1024-65535", "1024-65535", "1024-65535"), "Game Filter all matches upstream port defaults");
        check((new GameFilterSettings { Mode = GameFilterMode.Tcp, TcpPorts = " 1024-1934, 1936-65535 " }).Ranges() ==
            ("1024-1934,1936-65535", "1024-1934,1936-65535", "12"), "TCP-only mode normalizes custom ranges and leaves UDP disabled");
        check((new GameFilterSettings { Mode = GameFilterMode.Udp, UdpPorts = "50000-50100" }).Ranges() ==
            ("50000-50100", "12", "50000-50100"), "UDP-only mode maps legacy placeholder like upstream");
        foreach (var invalid in new[] { "", "0", "65536", "100-10", "01", "80,", ",80", "80,,443", "80\n", "80\t", "80 & calc", "%BIN%", "-80", "80-443-8443", new string('1', 2049) })
            Reject(() => GameFilterSettings.ValidatePorts(invalid), "Reject unsafe/invalid Game Filter ports: " + invalid[..Math.Min(invalid.Length, 24)]);
        Reject(() => (new GameFilterSettings { Mode = (GameFilterMode)999 }).Validate(), "Reject unknown Game Filter mode");
        check(GameIpSet.Validate("# comment\n1.2.3.0/24\n2001:db8::/32\n") == "1.2.3.0/24\n2001:db8::/32\n", "IPSet accepts IPv4 and IPv6 CIDR lists");
        foreach (var invalid in new[] { "", "203.0.113.113/32\n", "<html>blocked</html>", "0.0.0.0/0", "1.2.3.4/33", "2001:db8::/129", "fe80::1%1/64", "1.2/24", "1.2.3.4/24 --new", new string('x', 2 * 1024 * 1024 + 1) })
            Reject(() => GameIpSet.Validate(invalid), "Reject unusable IPSet: " + invalid[..Math.Min(invalid.Length, 24)]);

        var root = Path.Combine(Path.GetTempPath(), "svyaz-game-check-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(root);
        try
        {
            const string script = "start \"zapret\" /min \"%BIN%winws.exe\" --wf-tcp=443,%GameFilterTCP% --wf-udp=443,%GameFilterUDP% --filter-tcp=%GameFilter% --ipset=\"%LISTS%ipset-all.txt\" --ipset-exclude=\"%LISTS%ipset-exclude.txt\"";
            var cachedPath = Path.Combine(root, "ipset.txt");
            var tcp = new GameFilterSettings { Mode = GameFilterMode.Tcp, TcpPorts = "3000,4000-5000" };
            var parsed = Strategy.Parse(script, root, tcp, cachedPath);
            check(parsed.Contains("--wf-tcp=443,3000,4000-5000") && parsed.Contains("--wf-udp=443,12") && parsed.Contains("--filter-tcp=3000,4000-5000"), "Strategy substitutes configured Game Filter arguments directly");
            check(parsed.Contains("--ipset=" + cachedPath) && parsed.Contains("--ipset-exclude=" + Path.Combine(root, "lists", "ipset-exclude.txt")), "Game Filter replaces main IPSet but preserves exclusion lists");
            check(Strategy.Parse(script, root, new(), cachedPath).Contains("--ipset=" + Path.Combine(root, "lists", "ipset-all.txt")), "Disabled Game Filter preserves original package IPSet");
            var statePath = Path.Combine(root, "settings.json");
            File.WriteAllText(statePath, "{\"LastStrategy\":\"ALT\"}");
            check(AppState.Load(statePath).GameFilter.Mode == GameFilterMode.Disabled, "Legacy settings keep Game Filter disabled");
            new AppState { GameFilter = tcp }.Save(statePath);
            check(AppState.Load(statePath).GameFilter == tcp, "Game Filter mode and ports survive restart");

            var downloads = 0;
            var store = new GameIpSet(cachedPath, _ => { }, _ => { downloads++; return Task.FromResult("1.2.3.0/24\n"); });
            await store.EnsureAsync(default);
            await store.EnsureAsync(default);
            check(downloads == 1 && File.ReadAllText(cachedPath) == "1.2.3.0/24\n", "IPSet downloads once and reuses validated fresh cache");
            File.SetLastWriteTimeUtc(cachedPath, DateTime.UtcNow.AddHours(-7));
            await new GameIpSet(cachedPath, _ => { }, _ => throw new HttpRequestException("Offline")).EnsureAsync(default);
            check(File.ReadAllText(cachedPath) == "1.2.3.0/24\n", "Network failure keeps cached IPSet");
            await new GameIpSet(cachedPath, _ => { }, _ => Task.FromResult("<html>blocked</html>")).EnsureAsync(default);
            check(File.ReadAllText(cachedPath) == "1.2.3.0/24\n", "Malformed IPSet update preserves cached data");
            using var cancellation = new CancellationTokenSource();
            cancellation.Cancel();
            try { await store.EnsureAsync(cancellation.Token); throw new Exception("Expected cancellation"); }
            catch (OperationCanceledException) { check(true, "IPSet cancellation is propagated"); }
            var absent = Path.Combine(root, "missing.txt");
            try { await new GameIpSet(absent, _ => { }, _ => Task.FromResult("203.0.113.113/32")).EnsureAsync(default); throw new Exception("Expected missing IPSet failure"); }
            catch (IOException) { check(!File.Exists(absent), "No usable IPSet fails explicitly instead of activating a dummy filter"); }
        }
        finally { Directory.Delete(root, true); }
    }
}
