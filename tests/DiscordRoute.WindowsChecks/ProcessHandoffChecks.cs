using System.Diagnostics;
using System.IO;
using DiscordRoute.App;
using DiscordRoute.Core;

internal static class ProcessHandoffChecks
{
    public static async Task RunAsync(Action<bool, string> check)
    {
        var root = Path.Combine(Path.GetTempPath(), "svyaz-process-checks-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(root);
        try
        {
            foreach (var name in new[] { "winws", "winws2" })
            {
                var executable = Path.Combine(root, name + ".exe");
                File.Copy(Path.Combine(Environment.SystemDirectory, "cmd.exe"), executable);
                using var process = new ManagedProcess();
                ProcessStartInfo StartInfo() => new(executable, "/d /c pause")
                    { UseShellExecute = false, CreateNoWindow = true, RedirectStandardInput = true };
                process.Start(StartInfo());
                var waiting = WindowsRunner.EnsureNoExternalProcessAsync(default);
                check(!waiting.IsCompleted && process.IsRunning, name + ": process conflict waits for teardown instead of rejecting immediately");
                await process.StopAsync();
                await waiting;
                check(!process.IsRunning, name + ": completed teardown allows the next strategy");

                for (var i = 0; i < 10; i++)
                {
                    process.Start(StartInfo());
                    await process.StopAsync();
                    await WindowsRunner.EnsureNoExternalProcessAsync(default);
                }
                check(true, name + ": ten rapid managed handoffs leave no false external conflict");

                process.Start(StartInfo());
                try
                {
                    try
                    {
                        await WindowsRunner.EnsureNoExternalProcessAsync(default);
                        throw new Exception("Live external process accepted");
                    }
                    catch (ConnectionFailure ex)
                    {
                        check(process.IsRunning && ex.Message.Contains(name + ", PID ") &&
                            ConnectionHelp.Describe(ex, true).Message == ex.Message,
                            name + ": persistent external process is preserved and its PID is shown to the user");
                    }
                    using var cancellation = new CancellationTokenSource();
                    waiting = WindowsRunner.EnsureNoExternalProcessAsync(cancellation.Token);
                    cancellation.Cancel();
                    try { await waiting; throw new Exception("Process wait ignored cancellation"); }
                    catch (OperationCanceledException) { check(process.IsRunning, name + ": cancelling conflict wait never terminates the external process"); }
                }
                finally { await process.StopAsync(); }
            }
        }
        finally
        {
            for (var attempt = 0; ; attempt++)
            {
                try { Directory.Delete(root, true); break; }
                catch (Exception ex) when (attempt < 20 && ex is (IOException or UnauthorizedAccessException))
                { await Task.Delay(100); }
            }
        }
    }
}
