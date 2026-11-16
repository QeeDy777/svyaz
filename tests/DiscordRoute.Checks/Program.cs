using DiscordRoute.Core;
using DiscordRoute.App;
using System.IO.Compression;
using System.Security.Cryptography;
using System.Xml.Linq;

var checks = 0;
void Check(bool condition, string name)
{
    if (!condition) throw new Exception("FAIL: " + name);
    Console.WriteLine("PASS: " + name);
    checks++;
}
void Reject(Action action, string name)
{
    try { action(); }
    catch (InvalidDataException) { Check(true, name); return; }
    throw new Exception("FAIL (accepted invalid input): " + name);
}

ConvenienceChecks.Run(Check);
ServiceSelectionChecks.Run(Check);
RecoveryChecks.Run(Check);
QualityChecks.Run(Check);
await QualityChecks.RunAsync(Check);
IconChecks.Run(Check);
TelegramChecks.Run(Check);
AppUpdateChecks.Run(Check);
await YouTubeChecks.RunAsync(Check, args.Contains("--youtube-smoke"));
TelegramMigrationChecks.Run(Check);
await GameFilterChecks.RunAsync(Check);
var ui = new PresentationState();
Check(!ui.ShowDisconnect && ui.PrimaryActionLabel == "Подключить", "Idle primary action connects");
ui.IsBusy = true;
Check(ui.ShowDisconnect && ui.PrimaryActionLabel == "Отменить и отключить", "Busy primary action remains available to cancel");
ui.IsBusy = false;
ui.IsProcessRunning = true;
Check(ui.ShowDisconnect && ui.PrimaryActionLabel == "Отключить", "Running primary action stops even without verification");
ui.IsProcessRunning = false;
var uiNotifications = 0;
ui.PropertyChanged += (_, _) => uiNotifications++;
ui.IsProcessRunning = true;
Check(!ui.IsVerified && ui.Badge == "НЕ ПРОВЕРЕНО", "Running process is not displayed as verified");
ui.IsBusy = true;
Check(!ui.CanEdit && ui.StopLabel == "Отменить и отключить" && uiNotifications == 2, "Busy state disables editing and notifies bindings");
ui.ShowServiceResults(null, checking: true);
Check(ui.Voice.Kind == CheckKind.Checking && ui.YouTube.Kind == CheckKind.Checking, "Voice shows pending while probe runs");
ui.ShowServiceResults(new(new(true, "API OK"), new(true, "Gateway OK"), new(false, "Voice failed"), new(new(true, "site"), new(true, "video"))));
Check(ui.Api.Kind == CheckKind.Passed && ui.Voice.Kind == CheckKind.Failed && ui.Voice.Detail == "Voice failed", "UI preserves separate voice failure and diagnostics");
ui.ShowServiceResults(new(new(true, "API OK"), new(true, "Gateway OK"), new(true, "Voice OK"), new(new(true, "site"), new(false, "video blocked"))));
Check(ui.YouTube.Kind == CheckKind.Failed && ui.Voice.Kind == CheckKind.Passed && ui.YouTube.Detail.Contains("video blocked"), "UI keeps a separate YouTube failure and endpoint details");
ui.IsBusy = false;
ui.ShowServiceResults(null);
Check(ui.CanEdit && ui.Voice.Kind == CheckKind.Idle && ui.YouTube.Kind == CheckKind.Idle, "Cancellation clears pending results");
ui.IsVerified = true;
Check(ui.Heading == "Discord и YouTube\nотвечают" && ui.Badge == "ПРОВЕРКИ ПРОЙДЕНЫ", "Verified state has explicit success label");
var commandCalls = 0;
var command = new UiCommand(() => { commandCalls++; return Task.CompletedTask; }, () => ui.CanEdit);
ui.IsBusy = true; command.Execute(null);
ui.IsBusy = false; command.Execute(null);
Check(commandCalls == 1, "Disabled command ignores activation");

Check(DiscordProtocol.IsHello("{\"op\":8,\"d\":{\"heartbeat_interval\":41250}}", 8), "Voice Hello");
Check(!DiscordProtocol.IsHello("{\"op\":10,\"d\":{\"heartbeat_interval\":41250}}", 8), "Gateway is not voice");
Check(!DiscordProtocol.IsHello("{\"op\":8,\"d\":{\"heartbeat_interval\":0}}", 8), "Invalid heartbeat");
Check(!DiscordProtocol.IsHello("<html>blocked</html>", 8), "Block page is not success");
Check(!DiscordProtocol.IsHello("{\"op\":8,\"d\":[]}", 8), "Malformed Hello");
var voice = VoiceEndpoint.Parse("wss://c-ams01-abc123.discord.media:8443/");
Check(voice.Port == 8443 && voice.Query == "?v=8", "Voice endpoint preserves port");
Reject(() => VoiceEndpoint.Parse("discord.media.evil.test"), "Reject unrelated host");
Reject(() => VoiceEndpoint.Parse("https://example.com@c-ams01.discord.media"), "Reject credentials and wrong scheme");
Reject(() => VoiceEndpoint.Parse("discord.media"), "Root domain is not voice endpoint");
Check(VoiceEndpoint.Extract("[CONNECT] wss://c-ams01-abc123.discord.media:8443/").Single().Port == 8443, "Extract endpoint from log");

var root = Path.Combine(Path.GetTempPath(), "discord-route-check-" + Guid.NewGuid().ToString("N"));
Directory.CreateDirectory(root);
try
{
    var startupFailure = new InvalidOperationException("XAML wrapper", new IOException("Image decoder cause"));
    var startupLog = StartupFailure.Save(startupFailure, Path.Combine(root, "startup-log"));
    Check(startupLog != null && File.ReadAllText(startupLog).Contains("XAML wrapper") &&
        File.ReadAllText(startupLog).Contains("Image decoder cause"), "Startup report preserves nested exception cause");
    var blockedLogDirectory = Path.Combine(root, "not-a-directory");
    File.WriteAllText(blockedLogDirectory, "fixture");
    Check(StartupFailure.Save(startupFailure, blockedLogDirectory) == null, "Startup report write failure does not hide original error");
    var runtimeSource = Path.Combine(root, "runtime-source");
    var runtimeInstall = Path.Combine(root, "runtime-install");
    Directory.CreateDirectory(runtimeSource);
    foreach (var name in RuntimeInstallation.Files) File.WriteAllText(Path.Combine(runtimeSource, name), name);
    var sourceExe = Path.Combine(runtimeSource, "Связь.exe");
    var installedExe = RuntimeInstallation.Install(sourceExe, runtimeInstall);
    Check(RuntimeInstallation.Files.All(name => File.Exists(Path.Combine(Path.GetDirectoryName(installedExe)!, name))), "Autostart installation includes every WPF runtime sidecar");
    Check(RuntimeInstallation.Install(sourceExe, runtimeInstall) == installedExe, "Unchanged runtime reuses installed version");
    File.AppendAllText(sourceExe, "v2");
    var nextInstalledExe = RuntimeInstallation.Install(sourceExe, runtimeInstall);
    Check(nextInstalledExe != installedExe && File.ReadAllText(installedExe) == "Связь.exe", "New runtime installs separately and preserves previous version");
    File.Delete(Path.Combine(runtimeSource, RuntimeInstallation.Files[1]));
    try { RuntimeInstallation.Install(sourceExe, runtimeInstall); throw new Exception("Expected missing runtime failure"); }
    catch (FileNotFoundException) { Check(File.Exists(nextInstalledExe), "Incomplete runtime does not damage installed copy"); }
    File.AppendAllText(nextInstalledExe, "tampered");
    File.WriteAllText(Path.Combine(runtimeSource, RuntimeInstallation.Files[1]), RuntimeInstallation.Files[1]);
    try { RuntimeInstallation.Install(sourceExe, runtimeInstall); throw new Exception("Expected corruption failure"); }
    catch (IOException) { Check(true, "Corrupt installed runtime is rejected"); }

    var script = "@echo off\nstart \"zapret\" /min \"%BIN%winws.exe\" --wf-tcp=443,%GameFilterTCP% ^\n--hostlist=\"%LISTS%list-general.txt\"\n";
    var arguments = Strategy.Parse(script, root);
    Check(arguments.Count == 2 && arguments[0] == "--wf-tcp=443,12", "Parse continuation and game defaults");
    Check(arguments[1] == "--hostlist=" + Path.Combine(root, "lists", "list-general.txt"), "Quoted path stays one argument");
    Reject(() => Strategy.Parse(script + "powershell evil\n", root), "Reject extra commands");
    Reject(() => Strategy.Parse(script.Replace("443,%GameFilterTCP%", "%UNKNOWN%"), root), "Reject unknown variables");
    Reject(() => Strategy.Parse(script.Replace("443,%GameFilterTCP%", "443 & calc.exe"), root), "Reject shell operators");
    Reject(() => Strategy.Parse(script.Replace("list-general.txt", "../outside.txt"), root), "Reject resource traversal");
    Check(Strategy.Parse(script.Replace("443,%GameFilterTCP%", "^!"), root)[0] == "--wf-tcp=!", "BAT escaped exclamation");
    var data = new byte[] { 1, 2, 3 };
    PackageStore.VerifyDigest(data, "sha256:" + Convert.ToHexString(SHA256.HashData(data)));
    Reject(() => PackageStore.VerifyDigest(data, "sha256:" + new string('0', 64)), "Reject tampered download");
    foreach (var name in new[] { "../outside.txt", "/absolute.txt", "C:/evil.txt", "x/../../evil.txt", "bin/CON.txt", "bin/x:stream", "bin/trailing. " })
    {
        using var memory = new MemoryStream();
        using (var archive = new ZipArchive(memory, ZipArchiveMode.Create, true)) archive.CreateEntry(name);
        Reject(() => PackageStore.Extract(memory.ToArray(), root), "Reject ZIP " + name);
    }
    using (var memory = new MemoryStream())
    {
        using (var archive = new ZipArchive(memory, ZipArchiveMode.Create, true))
        {
            archive.CreateEntry("bin/WINWS.exe");
            archive.CreateEntry("bin/winws.exe");
        }
        Reject(() => PackageStore.Extract(memory.ToArray(), Path.Combine(root, "duplicates")), "Reject case-insensitive ZIP collision");
    }
    using (var memory = new MemoryStream())
    {
        using (var archive = new ZipArchive(memory, ZipArchiveMode.Create, true)) archive.CreateEntry("link").ExternalAttributes = unchecked((int)0xA1FF0000);
        Reject(() => PackageStore.Extract(memory.ToArray(), root), "Reject ZIP symlink");
    }
    var old = Path.Combine(root, "old");
    var next = Path.Combine(root, "next");
    PackageStore.PrepareLists(old, null);
    File.WriteAllText(Path.Combine(old, "lists", "list-general-user.txt"), "custom.example\n");
    PackageStore.PrepareLists(next, old);
    Check(File.ReadAllText(Path.Combine(next, "lists", "list-general-user.txt")) == "custom.example\n", "Preserve user lists");
    var statePath = Path.Combine(root, "settings.json");
    new AppState { LastStrategy = "ALT2", ActivePackage = "test" }.Save(statePath);
    Check(AppState.Load(statePath).LastStrategy == "ALT2", "State survives restart");
    var preferencesPath = Path.Combine(root, "preferences.json");
    new ConvenienceState { NetworkStrategies = new() { ["home"] = "ALT1" }, LastSeenAppVersion = "0.7.0" }.Save(preferencesPath);
    AppState.Load(statePath).Save(statePath);
    Check(ConvenienceState.Load(preferencesPath).NetworkStrategies["home"] == "ALT1" && ConvenienceState.Load(preferencesPath).LastSeenAppVersion == "0.7.0", "Older settings writer cannot erase network profiles or release history");
    var xml = XDocument.Parse(LogonTask.Xml(@"C:\Program Files\Test & Test\Связь.exe", "S-1-5-21-123"));
    XNamespace taskNs = "http://schemas.microsoft.com/windows/2004/02/mit/task";
    Check(xml.Descendants(taskNs + "Command").Single().Value.Contains("Test & Test"), "Autostart XML escapes executable");
    Check(xml.Descendants(taskNs + "LogonType").Single().Value == "InteractiveToken" &&
        xml.Descendants(taskNs + "ExecutionTimeLimit").Single().Value == "PT0S", "Interactive autostart has no three-day timeout");

    var good = new CheckResult(true, "OK");
    var pass = new ProbeResult(good, good, good, new(good, good));
    var fail = new ProbeResult(good, good, new(false, "No voice"), new(good, good));
    var a = new Strategy("A", "unused");
    var b = new Strategy("B", "unused");
    var fake = new FakeRunner();
    var calls = 0;
    var engine = new SelectionEngine(fake, _ => Task.FromResult(++calls == 1 ? fail : pass), (_, _, _) => { });
    var chosen = await engine.SelectAsync(new("one", "1", root), [a, b], "B", default);
    Check(chosen == a && fake.Started.SequenceEqual(["B", "A"]) && fake.IsRunning && calls == 3, "Prefer saved strategy, require voice and two passes");
    fake = new FakeRunner();
    calls = 0;
    var noVideo = pass with { YouTube = new(good, new(false, "Googlevideo blocked")) };
    engine = new SelectionEngine(fake, _ => Task.FromResult(++calls == 2 ? noVideo : pass), (_, _, _) => { });
    Check(await engine.SelectAsync(new("one", "1", root), [a, b], "A", default) == b && calls == 4,
        "Discord success cannot hide YouTube failure on the second pass; next ALT is tested twice");
    fake = new FakeRunner();
    engine = new SelectionEngine(fake, _ => Task.FromResult(noVideo), (_, _, _) => { });
    Check(await engine.SelectAsync(new("one", "1", root), [a], null, default) == null && !fake.IsRunning,
        "No ALT is retained when YouTube fails despite Discord success");
    fake = new FakeRunner();
    engine = new SelectionEngine(fake, _ => Task.FromResult(fail), (_, _, _) => { });
    Check(await engine.SelectAsync(new("one", "1", root), [a], null, default) == null && !fake.IsRunning, "All failed leaves no process");
    fake = new FakeRunner();
    using (var cancel = new CancellationTokenSource())
    {
        engine = new SelectionEngine(fake, token => { cancel.Cancel(); token.ThrowIfCancellationRequested(); return Task.FromResult(pass); }, (_, _, _) => { });
        try { await engine.SelectAsync(new("one", "1", root), [a], null, cancel.Token); throw new Exception("Expected cancellation"); }
        catch (OperationCanceledException) { Check(!fake.IsRunning, "Cancellation stops candidate"); }
    }
    File.WriteAllText(Path.Combine(old, "general.bat"), script);
    File.WriteAllText(Path.Combine(next, "general.bat"), script);
    var oldPackage = new Package("old", "1", old);
    var nextPackage = new Package("next", "2", next);
    fake = new FakeRunner();
    await fake.StartAsync(oldPackage, new("general", "unused"), default);
    var sessionState = new AppState { ActivePackage = "old", PendingPackage = "next", LastStrategy = "general" };
    var session = new ConnectionSession(fake, sessionState, () => { }, (_, _, _) => { }, _ => { });
    await session.ConnectAsync(oldPackage, nextPackage, _ => Task.FromResult(fake.CurrentPackage == "old" ? pass : fail), null, default);
    Check(sessionState.ActivePackage == "old" && sessionState.PendingPackage == null && fake.CurrentPackage == "old", "Failed update falls back to validated old package");
    sessionState.PendingPackage = "next";
    await session.ConnectAsync(oldPackage, nextPackage, _ => Task.FromResult(pass), null, default);
    Check(sessionState.ActivePackage == "next" && sessionState.PreviousPackage == "old", "Successful update promotes atomically");
    sessionState = new AppState { ActivePackage = "old", PendingPackage = "next", LastStrategy = "general" };
    fake = new FakeRunner();
    await fake.StartAsync(oldPackage, new("general", "unused"), default);
    session = new ConnectionSession(fake, sessionState, () => throw new IOException("Disk full"), (_, _, _) => { }, _ => { });
    try { await session.ConnectAsync(oldPackage, nextPackage, _ => Task.FromResult(pass), null, default); throw new Exception("Expected save failure"); }
    catch (IOException) { Check(sessionState.ActivePackage == "old" && fake.CurrentPackage == "old" && fake.IsRunning, "State save failure restores previous process"); }
    File.WriteAllText(Path.Combine(old, "general (ALT2).bat"), script);
    fake = new FakeRunner();
    sessionState = new AppState { ActivePackage = "old", LastStrategy = "general" };
    session = new ConnectionSession(fake, sessionState, () => { }, (_, _, _) => { }, _ => { });
    await session.ConnectAsync(oldPackage, null, _ => Task.FromResult(pass), null, default, "general (ALT2)");
    Check(fake.Started.First() == "general (ALT2)", "Connection uses current network preference before global last strategy");
    fake = new FakeRunner();
    calls = 0;
    session = new ConnectionSession(fake, sessionState, () => { }, (_, _, _) => { }, _ => { });
    await session.ConnectAsync(oldPackage, null, _ => Task.FromResult(++calls == 1 ? fail : pass), null, default, "general (ALT2)");
    Check(sessionState.LastStrategy == "general" && fake.Started.Count == 2, "Repair can replace a failing saved strategy with a twice-tested alternative");
    foreach (var n in Enumerable.Range(3, 8)) File.WriteAllText(Path.Combine(old, $"general (ALT{n}).bat"), script);
    fake = new FakeRunner();
    var saves = 0;
    session = new ConnectionSession(fake, sessionState, () => saves++, (_, _, _) => { }, _ => { });
    ProbeResult TimedSession()
    {
        var timed = new CheckResult(true, "OK", ElapsedMs: fake.Started[^1] == "general (ALT10)" ? 10 : 100);
        return new(timed, timed, timed, new(timed, timed));
    }
    await session.ConnectAsync(oldPackage, null, _ => Task.FromResult(TimedSession()), null, default, fullScan: true);
    Check(fake.Started.Distinct().Count() == 10 && sessionState.LastStrategy == "general (ALT10)" && saves == 1,
        "Connection session forwards full scan and saves the confirmed winner once");
    fake = new FakeRunner(); calls = 0;
    session = new ConnectionSession(fake, sessionState, () => { }, (_, _, _) => { }, _ => { });
    await session.ConnectAsync(oldPackage, null, _ => { calls++; return Task.FromResult(TimedSession()); }, "general", default, fullScan: true);
    Check(fake.Started.SequenceEqual(new[] { "general" }) && calls == 2, "An explicit manual strategy remains a single two-pass check in the session");
    await LocalProtocolChecks.RunAsync(Check);
    var fixtures = args.Where(a => !a.StartsWith("--", StringComparison.Ordinal)).ToArray();
    if (fixtures.Length > 0)
    {
        var files = Directory.GetFiles(fixtures[0], "general*.bat");
        Check(files.Length > 10, "Actual upstream fixture present");
        foreach (var file in files)
        {
            Check(Strategy.Parse(File.ReadAllText(file), fixtures[0]).Count > 10, Path.GetFileName(file));
            foreach (var mode in new[] { GameFilterMode.All, GameFilterMode.Tcp, GameFilterMode.Udp })
            {
                var parsed = Strategy.Parse(File.ReadAllText(file), fixtures[0], new() { Mode = mode }, Path.Combine(root, "game-ipset.txt"));
                Check(parsed.Any(argument => argument.Contains("1024-65535", StringComparison.Ordinal)) &&
                    parsed.Contains("--ipset=" + Path.Combine(root, "game-ipset.txt")), Path.GetFileName(file) + " / " + mode);
            }
        }
    }
    if (fixtures.Length > 1)
    {
        PackageStore.Extract(File.ReadAllBytes(fixtures[1]), Path.Combine(root, "official-release"));
        Check(Directory.GetFiles(Path.Combine(root, "official-release"), "winws.exe", SearchOption.AllDirectories).Length == 1, "Extract actual official ZIP");
    }
    if (args.Contains("--github-smoke"))
    {
        var store = new PackageStore(Path.Combine(root, "github-smoke"), Console.WriteLine);
        using var deadline = new CancellationTokenSource(TimeSpan.FromMinutes(3));
        var downloaded = await store.StageLatestAsync(null, null, deadline.Token);
        Check(downloaded != null && Strategy.Load(downloaded.Root, Console.WriteLine).Count > 10, "Live GitHub download, digest, extraction and strategies");
        Check(store.Open(downloaded!.Id)?.Version == downloaded.Version, "Reopen staged package");
        Check(await store.StageLatestAsync(downloaded.Version, downloaded.Root, deadline.Token) == null, "Same release does not redownload");
    }
    if (args.Contains("--ipset-smoke"))
    {
        var ipset = new GameIpSet(Path.Combine(root, "live-ipset", "ipset-all.txt"), Console.WriteLine);
        await ipset.EnsureAsync(default);
        Check(File.ReadLines(ipset.FilePath).Count() > 1000, "Live Flowseal IPSet download and CIDR validation");
    }
}
finally { Directory.Delete(root, true); }
Console.WriteLine($"{checks} checks passed.");

sealed class FakeRunner : IStrategyRunner
{
    public bool IsRunning { get; private set; }
    public string? CurrentPackage { get; private set; }
    public List<string> Started { get; } = [];
    public Task StartAsync(Package package, Strategy strategy, CancellationToken ct)
    {
        ct.ThrowIfCancellationRequested();
        IsRunning = true; CurrentPackage = package.Id; Started.Add(strategy.Name);
        return Task.CompletedTask;
    }
    public Task StopAsync() { IsRunning = false; return Task.CompletedTask; }
}
