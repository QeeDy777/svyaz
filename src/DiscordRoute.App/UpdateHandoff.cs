using System.Diagnostics;

namespace DiscordRoute.App;

internal sealed record UpdateRestart(InstalledApp App, string[] Arguments, bool Rollback = false);

internal static class UpdateHandoff
{
    public static bool Start(string executable, string[] arguments, Action ready, TimeSpan timeout)
    {
        var token = Guid.NewGuid().ToString("N");
        using var signal = new EventWaitHandle(false, EventResetMode.ManualReset, "Local\\Svyaz-Update-" + token);
        var start = new ProcessStartInfo(executable) { UseShellExecute = false, WorkingDirectory = Path.GetDirectoryName(executable)! };
        foreach (var argument in arguments) start.ArgumentList.Add(argument);
        start.ArgumentList.Add("--update-ready"); start.ArgumentList.Add(token);
        using var process = Process.Start(start) ?? throw new IOException("Не удалось запустить обновлённую «Связь».");
        try
        {
            var timer = Stopwatch.StartNew();
            while (timer.Elapsed < timeout)
            {
                if (process.HasExited) return false;
                if (signal.WaitOne(100))
                {
                    if (process.HasExited) return false;
                    ready();
                    return true;
                }
            }
        }
        catch { Stop(process); throw; }
        Stop(process);
        return false;
    }

    public static void Signal(string token)
    {
        if (!Guid.TryParseExact(token, "N", out _)) throw new IOException("Неверное подтверждение обновления.");
        using var signal = EventWaitHandle.OpenExisting("Local\\Svyaz-Update-" + token);
        signal.Set();
    }

    private static void Stop(Process process)
    {
        if (process.HasExited) return;
        try { process.Kill(entireProcessTree: true); }
        catch (InvalidOperationException) when (process.HasExited) { }
        if (!process.WaitForExit(5000)) throw new IOException("Не удалось завершить неответившую версию «Связи».");
    }
}
