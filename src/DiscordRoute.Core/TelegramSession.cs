namespace DiscordRoute.Core;

public interface ITelegramRunner
{
    bool IsRunning { get; }
    Task StartAsync(Package package, TelegramSettings settings, CancellationToken ct);
    Task StopAsync();
}

public sealed class TelegramSession(ITelegramRunner runner, AppState state, Action save, Action<string> log)
{
    public async Task<Package> StartAsync(Package? active, Package? pending, CancellationToken ct)
    {
        var candidate = pending ?? active ?? throw new IOException("Сначала загрузите Telegram-компонент.");
        var oldActive = state.TelegramActivePackage;
        var oldPending = state.TelegramPendingPackage;
        try
        {
            ct.ThrowIfCancellationRequested();
            state.Telegram.WriteConfiguration(candidate.Root, active?.Root);
            await runner.StartAsync(candidate, state.Telegram, ct);
            ct.ThrowIfCancellationRequested();
            state.TelegramActivePackage = candidate.Id;
            state.TelegramPendingPackage = null;
            save();
            return candidate;
        }
        catch (Exception ex)
        {
            state.TelegramActivePackage = oldActive;
            state.TelegramPendingPackage = oldPending;
            await runner.StopAsync();
            if (ct.IsCancellationRequested || ex is OperationCanceledException || pending == null || active == null) throw;
            log("Telegram: новая версия не запустилась; пробуем предыдущую. " + ex.Message);
            state.Telegram.WriteConfiguration(active.Root);
            await runner.StartAsync(active, state.Telegram, ct);
            return active;
        }
    }
}
