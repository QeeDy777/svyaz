using System.Diagnostics;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal sealed class WindowsRunner(Action<string> log, Func<GameFilterSettings> gameFilter, string gameIpSetPath) : IStrategyRunner, IDisposable
{
    private readonly ManagedProcess process = new();
    private DateTimeOffset? startedAt;
    public DateTimeOffset? StartedAt => IsRunning ? startedAt : null;
    public string? StrategyName { get; private set; }
    public bool IsRunning => process.IsRunning;

    public static async Task EnsureNoExternalProcessAsync(CancellationToken ct)
    {
        for (var attempt = 0; ; attempt++)
        {
            ct.ThrowIfCancellationRequested();
            var conflict = FindRunningProcess();
            if (conflict == null) return;
            if (attempt == 8)
                throw new ConnectionFailure("Обнаружен работающий zapret (" + conflict + "). Остановите отдельный zapret или его службу перед подбором. «Связь» не завершает чужие процессы.");
            // A stopped process or its job children may still appear during Windows teardown.
            await Task.Delay(250, ct);
        }
    }

    private static string? FindRunningProcess()
    {
        foreach (var name in new[] { "winws", "winws2" })
        {
            var processes = Process.GetProcessesByName(name);
            try
            {
                foreach (var item in processes)
                    if (!item.HasExited) return name + ", PID " + item.Id;
            }
            finally { foreach (var item in processes) item.Dispose(); }
        }
        return null;
    }

    public async Task StartAsync(Package package, Strategy strategy, CancellationToken ct)
    {
        ct.ThrowIfCancellationRequested();
        await StopAsync();
        await EnsureNoExternalProcessAsync(ct);
        var start = new ProcessStartInfo(Path.Combine(package.Root, "bin", "winws.exe"))
        {
            WorkingDirectory = Path.Combine(package.Root, "bin"), UseShellExecute = false,
            CreateNoWindow = true, RedirectStandardError = true, RedirectStandardOutput = true
        };
        foreach (var argument in Strategy.Parse(File.ReadAllText(strategy.FilePath), package.Root, gameFilter(), gameIpSetPath)) start.ArgumentList.Add(argument);
        try
        {
            process.Start(start, line => log("winws: " + line));
            startedAt = DateTimeOffset.UtcNow;
            StrategyName = strategy.Name;
            await Task.Delay(1200, ct);
            if (!process.IsRunning) throw new IOException("winws завершился (код " + process.ExitCode + "). См. журнал.");
        }
        catch { await StopAsync(); throw; }
    }

    public Task StopAsync() => process.StopAsync();
    public void Dispose() => process.Dispose();
}
