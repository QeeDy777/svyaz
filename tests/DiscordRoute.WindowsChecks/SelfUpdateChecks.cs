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

    public static void Run(string? archivePath, Action<bool, string> check)
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
            updater.Reject(new InstalledApp("99.0.0", new string('a', 64)));
            check(updater.Read("current") == candidate, "Rejecting a failed candidate preserves the previous current version");
            updater.Reject(candidate);
            check(updater.Read("current") == null && updater.Read("rejected") == candidate && File.Exists(installedExe), "Failed startup rejects the candidate but preserves runtime files");
            if (Environment.GetEnvironmentVariable("GITHUB_ACTIONS") == "true") CheckRealWindow(installedExe, check);
            var pointer = Path.Combine(data, "AppUpdate", "current.json");
            File.WriteAllText(pointer, "{\"Version\":\"0.5.0\",\"Id\":\"../evil\"}");
            try { updater.Read("current"); throw new Exception("Unsafe update pointer accepted"); }
            catch (InvalidDataException) { check(true, "Reject unsafe installed update pointer"); }
        }
        finally { if (Directory.Exists(root)) Directory.Delete(root, true); }
    }

    private static void CheckRealWindow(string installedExe, Action<bool, string> check)
    {
        // Only the disposable CI machine may use the application's real Program Files data directory.
        var settings = Path.Combine(DiscordRoute.App.Program.DataRoot, "settings.json");
        if (File.Exists(settings)) throw new IOException("Self-update CI requires an unused settings path");
        new AppState { AutoUpdate = false, AutostartConfigured = true }.Save(settings);
        try
        {
            try
            {
                UpdateHandoff.Start(installedExe, [], () => throw new CommitFailure(), TimeSpan.FromSeconds(30));
                throw new Exception("Real published application did not acknowledge startup");
            }
            catch (CommitFailure) { check(true, "Published EXE opens its window; failed promotion terminates the candidate for rollback"); }
            check(!System.Text.Json.JsonSerializer.Deserialize<AppState>(File.ReadAllText(settings))!.AutoUpdate,
                "Restart preserves disabled updates and existing settings");
        }
        finally { File.Delete(settings); }
    }

    private sealed class CommitFailure : Exception;
}
