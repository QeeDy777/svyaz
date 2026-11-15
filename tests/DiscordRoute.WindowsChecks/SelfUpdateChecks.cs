using System.Diagnostics;
using System.IO;
using System.Security.Cryptography;
using DiscordRoute.App;
using DiscordRoute.Core;

internal static class SelfUpdateChecks
{
    public static int Probe(string[] args)
    {
        if (args.Contains("--fail")) return 7;
        if (!args.Contains("--hang")) UpdateHandoff.Signal(args[Array.IndexOf(args, "--update-ready") + 1]);
        using var done = EventWaitHandle.OpenExisting(args[Array.IndexOf(args, "--done") + 1]);
        return done.WaitOne(10000) ? 0 : 8;
    }

    public static void Run(string? archivePath, Action<bool, string> check, string? olderArchivePath = null)
    {
        var doneName = "Local\\Svyaz-Test-" + Guid.NewGuid().ToString("N");
        using var done = new EventWaitHandle(false, EventResetMode.ManualReset, doneName);
        var exe = Environment.ProcessPath!;
        var arguments = new[] { "--handoff-probe", "--done", doneName };
        var promoted = false;
        check(UpdateHandoff.Start(exe, arguments, () => { promoted = true; done.Set(); }, TimeSpan.FromSeconds(10)) && promoted,
            "Self-update waits for child startup acknowledgement before promotion");
        check(!UpdateHandoff.Start(exe, arguments.Append("--fail").ToArray(), () => throw new Exception("Must not promote"), TimeSpan.FromSeconds(10)),
            "Self-update rejects a process that exits before readiness");
        done.Reset();
        check(!UpdateHandoff.Start(exe, arguments.Append("--hang").ToArray(), () => throw new Exception("Must not promote"), TimeSpan.FromMilliseconds(500)),
            "Self-update terminates a child that never acknowledges startup");
        if (archivePath == null) return;
        var root = Path.Combine(Path.GetTempPath(), "svyaz-selfupdate-" + Guid.NewGuid().ToString("N"));
        var data = Path.Combine(root, "Data");
        try
        {
            Directory.CreateDirectory(data);
            var sentinel = Path.Combine(data, "settings.json");
            File.WriteAllText(sentinel, "existing settings");
            var bytes = File.ReadAllBytes(archivePath);
            var release = new AppRelease(AppUpdater.RunningVersion, new Uri("https://github.com/QeeDy777/svyaz"),
                "sha256:" + Convert.ToHexStringLower(SHA256.HashData(bytes)), bytes.Length);
            var updater = new AppUpdater(root, data);
            var candidate = updater.Stage(release, bytes, default);
            var installedExe = updater.Executable(candidate);
            check(updater.Read("pending") == candidate && File.ReadAllText(sentinel) == "existing settings", "Real archive installed without touching settings");
            var dll = Path.Combine(Path.GetDirectoryName(installedExe)!, "PenImc_cor3.dll");
            var original = File.ReadAllBytes(dll);
            File.WriteAllText(dll, "corrupt");
            try { updater.Executable(candidate); throw new Exception("Tampered runtime accepted"); }
            catch (IOException) { check(true, "Self-update verifies the entire installed runtime before launch"); }
            File.WriteAllBytes(dll, original);
            try { updater.Stage(release with { Version = new Version(99, 0, 0) }, bytes, default); throw new Exception("Version mismatch accepted"); }
            catch (InvalidDataException) { check(updater.Read("pending") == candidate, "Wrong EXE version cannot replace a prepared update"); }
            updater.Promote(candidate);
            check(updater.Read("current") == candidate && updater.Read("pending") == null, "Successful startup promotes update and clears pending state");
            if (olderArchivePath != null) CheckRollback(updater, candidate, olderArchivePath, data, check);
            updater.Reject(new InstalledApp("99.0.0", new string('a', 64)));
            check(updater.Read("current") == candidate, "Rejecting a failed candidate preserves the previous current version");
            updater.Reject(candidate);
            check(updater.Read("current") == null && updater.Read("rejected") == candidate && File.Exists(installedExe), "Failed startup rejects the candidate but preserves runtime files");
            if (Environment.GetEnvironmentVariable("GITHUB_ACTIONS") == "true") CheckRealWindow(installedExe, check, telegram: true);
            var pointer = Path.Combine(data, "AppUpdate", "current.json");
            File.WriteAllText(pointer, "{\"Version\":\"0.5.0\",\"Id\":\"../evil\"}");
            try { updater.Read("current"); throw new Exception("Unsafe update pointer accepted"); }
            catch (InvalidDataException) { check(true, "Reject unsafe installed update pointer"); }
        }
        finally { if (Directory.Exists(root)) Directory.Delete(root, true); }
    }

    private static void CheckRollback(AppUpdater updater, InstalledApp candidate, string olderArchivePath, string data, Action<bool, string> check)
    {
        var bytes = File.ReadAllBytes(olderArchivePath);
        var old = updater.Stage(new(new Version(0, 6, 1), new Uri("https://github.com/QeeDy777/svyaz"),
            "sha256:" + Convert.ToHexStringLower(SHA256.HashData(bytes)), bytes.Length), bytes, default);
        check(AppUpdater.PreviousVersion(updater.InstalledVersions(), AppUpdater.RunningVersion) == old,
            "Rollback discovers and verifies a real older runtime left by 0.6 updater");
        check(AppUpdater.PreviousVersion([old with { Version = "0.4.0" }], AppUpdater.RunningVersion) == null,
            "Rollback excludes releases without startup acknowledgement support");
        var blocked = Path.Combine(data, "AppUpdate", "current.json.tmp");
        Directory.CreateDirectory(blocked);
        try
        {
            try { updater.Promote(old, true); throw new Exception("Blocked rollback commit accepted"); }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
            {
                check(updater.Read("current") == candidate && updater.SelectForLaunch(false) == null,
                    "Failed rollback commit preserves current version even after rollback intent was written");
            }
        }
        finally { Directory.Delete(blocked); }
        updater.Promote(old, true);
        check(updater.Read("current") == old && updater.Read("pending") == null && updater.SelectForLaunch(false) == old,
            "Rollback persists older selection for a newer installed launcher and removes pending automatic upgrade");
        check(File.ReadAllText(Path.Combine(data, "settings.json")) == "existing settings", "Rollback promotion preserves settings");
        if (Environment.GetEnvironmentVariable("GITHUB_ACTIONS") == "true") CheckRealWindow(updater.Executable(old), check);
        Directory.CreateDirectory(blocked);
        try
        {
            try { updater.Promote(candidate); throw new Exception("Blocked upgrade commit accepted"); }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
            {
                check(updater.Read("current") == old && updater.SelectForLaunch(false) == old,
                    "Failed upgrade commit preserves the previously selected rollback version");
            }
        }
        finally { Directory.Delete(blocked); }
        updater.SelectForLaunch(true);
        check(updater.Read("rollback") == null, "Explicitly re-enabling automatic updates releases rollback selection");
        updater.Promote(candidate);
        check(updater.Read("rollback") == null && updater.Read("current") == candidate, "Explicit upgrade clears rollback selection");
    }

    private static void CheckRealWindow(string installedExe, Action<bool, string> check, bool telegram = false)
    {
        // Only the disposable CI machine may use the application's real Program Files data directory.
        var settings = Path.Combine(DiscordRoute.App.Program.DataRoot, "settings.json");
        if (File.Exists(settings)) throw new IOException("Self-update CI requires an unused settings path");
        var telegramSettings = new TelegramSettings().WithSecret();
        if (telegram)
        {
            var listener = new System.Net.Sockets.TcpListener(System.Net.IPAddress.Loopback, 0);
            listener.Start();
            telegramSettings = telegramSettings with { Port = ((System.Net.IPEndPoint)listener.LocalEndpoint).Port };
            listener.Stop();
        }
        var config = TelegramSettings.ConfigurationPath(Path.Combine(DiscordRoute.App.Program.DataRoot, "Telegram"));
        var previousConfig = File.Exists(config) ? File.ReadAllBytes(config) : null;
        new AppState { AutoUpdate = false, AutostartConfigured = true, Telegram = telegramSettings }.Save(settings);
        if (telegram)
        {
            Directory.CreateDirectory(Path.GetDirectoryName(config)!);
            File.WriteAllText(config, telegramSettings.Configuration("{\"pool_size\":0,\"cfproxy\":false}"));
        }
        try
        {
            try
            {
                UpdateHandoff.Start(installedExe, telegram ? ["--resume-telegram"] : [], () =>
                {
                    if (telegram)
                    {
                        var deadline = DateTime.UtcNow.AddSeconds(15);
                        var connected = false;
                        while (!connected && DateTime.UtcNow < deadline)
                        {
                            using var client = new System.Net.Sockets.TcpClient();
                            try
                            {
                                client.ConnectAsync(System.Net.IPAddress.Loopback, telegramSettings.Port).WaitAsync(TimeSpan.FromMilliseconds(300)).GetAwaiter().GetResult();
                                connected = true;
                            }
                            catch (Exception ex) when (ex is System.Net.Sockets.SocketException or TimeoutException) { Thread.Sleep(100); }
                        }
                        check(connected, "Published single-file EXE starts its embedded Telegram engine after update handoff");
                    }
                    throw new CommitFailure();
                }, TimeSpan.FromSeconds(30));
                throw new Exception("Real published application did not acknowledge startup");
            }
            catch (CommitFailure) { check(true, "Published EXE opens its window; failed promotion terminates the candidate for rollback"); }
            check(!System.Text.Json.JsonSerializer.Deserialize<AppState>(File.ReadAllText(settings))!.AutoUpdate,
                "Restart preserves disabled updates and existing settings");
        }
        finally
        {
            File.Delete(settings);
            if (telegram)
            {
                if (previousConfig == null) File.Delete(config);
                else File.WriteAllBytes(config, previousConfig);
            }
        }
    }

    private sealed class CommitFailure : Exception;
}
