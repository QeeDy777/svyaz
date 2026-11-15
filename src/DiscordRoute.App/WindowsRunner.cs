using System.Diagnostics;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal sealed class WindowsRunner(Action<string> log, Func<GameFilterSettings> gameFilter, string gameIpSetPath) : IStrategyRunner, IDisposable
{
    private readonly ManagedProcess process = new();
    public bool IsRunning => process.IsRunning;

    public static void EnsureNoExternalProcess()
    {
        foreach (var name in new[] { "winws", "winws2" })
        {
            var processes = Process.GetProcessesByName(name);
            var any = processes.Length > 0;
            foreach (var item in processes) item.Dispose();
            if (any) throw new InvalidOperationException("Уже работает другой zapret (" + name + "). Остановите его перед подбором; приложение не будет завершать чужой процесс.");
        }
    }

    public async Task StartAsync(Package package, Strategy strategy, CancellationToken ct)
    {
        ct.ThrowIfCancellationRequested();
        await StopAsync();
        EnsureNoExternalProcess();
        var start = new ProcessStartInfo(Path.Combine(package.Root, "bin", "winws.exe"))
        {
            WorkingDirectory = Path.Combine(package.Root, "bin"), UseShellExecute = false,
            CreateNoWindow = true, RedirectStandardError = true, RedirectStandardOutput = true
        };
        foreach (var argument in Strategy.Parse(File.ReadAllText(strategy.FilePath), package.Root, gameFilter(), gameIpSetPath)) start.ArgumentList.Add(argument);
        try
        {
            process.Start(start, line => log("winws: " + line));
            await Task.Delay(1200, ct);
            if (!process.IsRunning) throw new IOException("winws завершился (код " + process.ExitCode + "). См. журнал.");
        }
        catch { await StopAsync(); throw; }
    }

    public Task StopAsync() => process.StopAsync();
    public void Dispose() => process.Dispose();
}
