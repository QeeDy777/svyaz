using DiscordRoute.Core;

namespace DiscordRoute.App;

internal sealed class AppUpdateViewModel : ObservableState, IDisposable
{
    private readonly AppUpdater updater = new();
    private readonly AppState state;
    private readonly Action<string> log;
    private CancellationTokenSource? cancellation;
    private Task? operation;
    private DateTimeOffset nextCheck;
    private bool exiting;
    private InstalledApp? pending;
    private string? announcedId, dismissedId;
    public bool IsNoticeVisible => pending != null && pending.Id != dismissedId && !exiting;
    public string NoticeTitle => "Версия " + pending?.Version + " готова";
    public string NoticeDetail { get; private set => Set(ref field, value); } = "Перезапуск ненадолго прервёт подключения.";
    public event Action<string>? UpdateReady;
    public UiCommand DismissCommand { get; }
    public string Version => AppUpdater.RunningVersion.ToString();
    public string Status { get; private set => Set(ref field, value); } = "Проверяем стабильные релизы GitHub. Настройки сохраняются.";
    public bool IsBusy { get; private set; }
    public UiCommand CheckCommand { get; }
    public UiCommand RestartCommand { get; }

    public AppUpdateViewModel(AppState state, Action<string> log, Action<InstalledApp> restart)
    {
        this.state = state; this.log = log;
        CheckCommand = new(() => CheckAsync(true), () => !IsBusy && !exiting);
        DismissCommand = new(() => { dismissedId = pending?.Id; Refresh(); return Task.CompletedTask; }, () => pending != null);
        RestartCommand = new(() =>
        {
            try { if (pending != null) { updater.Executable(pending); restart(pending); } }
            catch (Exception ex) { Status = NoticeDetail = ex.Message; log("Обновление «Связи»: " + ex.Message); }
            return Task.CompletedTask;
        }, () => pending != null && !IsBusy && !exiting);
        try { SetPending(updater.Read("pending")); }
        catch (Exception ex) { Status = ex.Message; }
    }

    public Task TickAsync() => state.AutoUpdate && DateTimeOffset.UtcNow >= nextCheck ? CheckAsync(false) : Task.CompletedTask;

    private Task CheckAsync(bool force)
    {
        if (IsBusy || exiting) return Task.CompletedTask;
        operation = CheckCoreAsync(force);
        return operation;
    }

    private async Task CheckCoreAsync(bool force)
    {
        using var source = new CancellationTokenSource();
        cancellation = source;
        IsBusy = true; Refresh();
        nextCheck = DateTimeOffset.UtcNow.AddHours(6);
        Status = "Проверяем и загружаем обновление «Связи»…";
        try
        {
            var prepared = await updater.PrepareAsync(force, source.Token);
            if (exiting) return;
            Status = "Установлена актуальная версия «Связи».";
            SetPending(prepared);
            log(Status);
        }
        catch (OperationCanceledException) { Status = "Обновление «Связи» отменено."; }
        catch (Exception ex) { Status = "Обновление «Связи»: " + ex.Message; log(Status); }
        finally { cancellation = null; IsBusy = false; Refresh(); }
    }

    public void SettingsChanged() => SetPending(pending);

    internal void SetPending(InstalledApp? update)
    {
        pending = update != null && AppUpdatePackage.ParseVersion(update.Version) > AppUpdater.RunningVersion ? update : null;
        if (pending != null) Status = "Версия " + pending.Version + " готова. " +
            (state.AutoUpdate ? "Применится при следующем запуске или по кнопке ниже." : "Нажмите «Обновить и перезапустить», чтобы установить её.");
        if (!exiting && pending != null && announcedId != pending.Id)
        {
            announcedId = pending.Id;
            NoticeDetail = "Перезапуск ненадолго прервёт подключения.";
            Refresh();
            UpdateReady?.Invoke(NoticeTitle);
        }
        else Refresh();
    }

    private void Refresh() { Changed(null); CheckCommand.Refresh(); RestartCommand.Refresh(); DismissCommand.Refresh(); }
    public async Task ExitAsync()
    {
        exiting = true;
        Refresh();
        cancellation?.Cancel();
        if (operation != null) await operation;
    }
    public void Dispose() { exiting = true; cancellation?.Cancel(); }
}
