namespace DiscordRoute.Core;

public sealed class ConnectionSession(IStrategyRunner runner, AppState state, Action save,
    Action<Strategy, ProbeResult?, string> report, Action<string> log)
{
    public async Task<Strategy> ConnectAsync(Package? active, Package? pending,
        Func<CancellationToken, Task<ProbeResult>> probe, string? manualStrategy, CancellationToken ct)
    {
        var wasRunning = runner.IsRunning;
        var oldStrategy = state.LastStrategy;
        var oldActive = state.ActivePackage;
        var oldPending = state.PendingPackage;
        var oldPrevious = state.PreviousPackage;
        var engine = new SelectionEngine(runner, probe, report);
        try
        {
            foreach (var package in new[] { pending, active }.OfType<Package>().DistinctBy(p => p.Id))
            {
                var all = Strategy.Load(package.Root, log);
                var strategies = manualStrategy == null ? all : all.Where(s => s.Name == manualStrategy).ToArray();
                if (strategies.Count == 0) continue;
                log("Проверка пакета " + package.Version);
                var selected = await engine.SelectAsync(package, strategies, oldStrategy, ct);
                if (selected == null) continue;
                if (package.Id != oldActive) state.PreviousPackage = oldActive;
                state.ActivePackage = package.Id;
                state.PendingPackage = null;
                state.LastStrategy = selected.Name;
                save();
                if (pending != null && package.Id != pending.Id) log("Новый пакет не прошёл проверку. Используется предыдущий.");
                return selected;
            }
            throw new IOException("Ни одна стратегия не прошла все проверки Discord дважды. Рабочий альт не найден.");
        }
        catch
        {
            state.ActivePackage = oldActive;
            state.PendingPackage = oldPending;
            state.PreviousPackage = oldPrevious;
            state.LastStrategy = oldStrategy;
            await runner.StopAsync();
            if (wasRunning && active != null && oldStrategy != null)
            {
                try
                {
                    var restore = Strategy.Load(active.Root, log).FirstOrDefault(s => s.Name == oldStrategy);
                    if (restore != null)
                    {
                        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(10));
                        await runner.StartAsync(active, restore, timeout.Token);
                        log("Восстановлен предыдущий процесс " + oldStrategy + ". Текущая доступность Discord не подтверждена.");
                    }
                }
                catch (Exception ex) { log("Не удалось восстановить процесс: " + ex.Message); }
            }
            throw;
        }
    }
}
