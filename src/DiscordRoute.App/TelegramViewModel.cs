using System.Diagnostics;
using System.Globalization;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal sealed class TelegramViewModel : ObservableState, IDisposable
{
    private readonly AppState state;
    private readonly Action save;
    private readonly Action<string> log;
    private readonly TelegramPackageStore packages;
    private readonly TelegramRunner runner;
    private CancellationTokenSource? cancellation;
    private TaskCompletionSource? operationDone;
    private DateTimeOffset nextUpdate;
    private bool exiting;
    public string Port { get; set; }
    public bool StartWithApp { get; set; }
    public bool IsBusy { get; private set; }
    public bool IsRunning { get; private set; }
    public bool CanEdit => !IsBusy && !IsRunning;
    public string PrimaryLabel => IsBusy ? "Отменить" : IsRunning ? "Остановить прокси" : "Запустить прокси";
    public string Status { get; private set => Set(ref field, value); } = "Telegram работает отдельно от подключения Discord.";
    public string Version { get; private set => Set(ref field, value); } = "Компонент ещё не загружен";
    public string Endpoint => "127.0.0.1:" + state.Telegram.Port;
    public UiCommand PrimaryCommand { get; }
    public UiCommand SaveCommand { get; }
    public UiCommand UpdateCommand { get; }
    public UiCommand OpenCommand { get; }
    public UiCommand CopyCommand { get; }

    public TelegramViewModel(AppState state, Action save, Action<string> log)
    {
        this.state = state; this.save = save; this.log = log;
        runner = new(log);
        state.Telegram = (state.Telegram ?? throw new InvalidDataException("Повреждены настройки Telegram.")).Validate();
        Port = state.Telegram.Port.ToString(CultureInfo.InvariantCulture);
        StartWithApp = state.Telegram.StartWithApp;
        packages = new(Path.Combine(Program.DataRoot, "Telegram", "Packages"), log);
        PrimaryCommand = new(ToggleAsync, () => !exiting);
        SaveCommand = new(() => RunAsync(_ => { SaveSettings(); Status = "Настройки Telegram сохранены."; return Task.CompletedTask; }), () => CanEdit && !exiting);
        UpdateCommand = new(() => RunAsync(ct => UpdateAsync(true, ct)), () => CanEdit && !exiting);
        OpenCommand = new(() => RunAsync(_ =>
        {
            Process.Start(new ProcessStartInfo(state.Telegram.ProxyUrl()) { UseShellExecute = true });
            Status = "Подтвердите подключение прокси в Telegram.";
            return Task.CompletedTask;
        }), () => IsRunning && !IsBusy && !exiting);
        CopyCommand = new(() => RunAsync(_ =>
        {
            System.Windows.Clipboard.SetText(state.Telegram.ProxyUrl());
            Status = "Ссылка скопирована. Не публикуйте её: в ней ключ локального прокси.";
            return Task.CompletedTask;
        }), () => IsRunning && !IsBusy && !exiting);
    }

    public Task InitializeAsync() => RunAsync(async ct =>
    {
        RefreshVersion();
        if (state.Telegram.StartWithApp) await StartAsync(ct);
    });

    public async Task TickAsync()
    {
        if (IsBusy || exiting) return;
        if (IsRunning && !runner.IsRunning) { Status = "Telegram-прокси остановился. Запустите его снова или отключите прокси в Telegram."; Refresh(); }
        if (!IsRunning && state.AutoUpdate && DateTimeOffset.UtcNow >= nextUpdate &&
            (state.TelegramActivePackage != null || state.TelegramPendingPackage != null))
            await RunAsync(ct => UpdateAsync(false, ct));
    }

    private Task ToggleAsync()
    {
        if (IsBusy) { cancellation?.Cancel(); return Task.CompletedTask; }
        return RunAsync(async ct =>
        {
            if (runner.IsRunning)
            {
                await runner.StopAsync();
                Status = "Прокси остановлен. При необходимости выключите его в настройках Telegram.";
                log("Telegram: прокси остановлен.");
            }
            else await StartAsync(ct);
        });
    }

    private void SaveSettings()
    {
        if (!int.TryParse(Port, NumberStyles.None, CultureInfo.InvariantCulture, out var port))
            throw new InvalidDataException("Укажите целый номер порта от 1 до 65535.");
        var previous = state.Telegram;
        var next = (previous with { Port = port, StartWithApp = StartWithApp }).WithSecret();
        try { state.Telegram = next; save(); }
        catch { state.Telegram = previous; throw; }
        Changed(null);
    }

    private async Task StartAsync(CancellationToken ct)
    {
        SaveSettings();
        var active = packages.Open(state.TelegramActivePackage);
        var pending = packages.Open(state.TelegramPendingPackage);
        if ((active == null && pending == null) || (state.AutoUpdate && DateTimeOffset.UtcNow >= nextUpdate))
        {
            try { await UpdateAsync(false, ct); }
            catch (Exception ex) when (!ct.IsCancellationRequested && (active != null || pending != null))
            { log("Telegram: обновление недоступно, используем сохранённый компонент. " + ex.Message); }
            active = packages.Open(state.TelegramActivePackage);
            pending = packages.Open(state.TelegramPendingPackage);
        }
        Status = "Запускаем локальный прокси…";
        var selected = await new TelegramSession(runner, state, save, log).StartAsync(active, pending, ct);
        Status = "Локальный прокси готов. Нажмите «Открыть в Telegram» и подтвердите подключение.";
        Version = "tg-ws-proxy " + selected.Version;
        log("Telegram: локальный порт отвечает; доступность сообщений и звонков не проверялась.");
    }

    private async Task UpdateAsync(bool force, CancellationToken ct)
    {
        if (runner.IsRunning) return;
        nextUpdate = DateTimeOffset.UtcNow.AddHours(6);
        var current = packages.Open(state.TelegramPendingPackage) ?? packages.Open(state.TelegramActivePackage);
        Status = "Проверяем Telegram-компонент на GitHub…";
        var newer = await packages.StageLatestAsync(force ? null : current?.Version, ct);
        if (newer == null) { Status = "Telegram-компонент актуален."; return; }
        var old = state.TelegramPendingPackage;
        try { state.TelegramPendingPackage = newer.Id; save(); }
        catch { state.TelegramPendingPackage = old; throw; }
        RefreshVersion();
        Status = "Компонент загружен и проверен. Он применится при следующем запуске прокси.";
    }

    private void RefreshVersion()
    {
        var package = packages.Open(state.TelegramPendingPackage) ?? packages.Open(state.TelegramActivePackage);
        Version = package == null ? "Компонент ещё не загружен" : "tg-ws-proxy " + package.Version;
    }

    private async Task RunAsync(Func<CancellationToken, Task> action)
    {
        if (IsBusy || exiting) return;
        using var operation = new CancellationTokenSource();
        cancellation = operation;
        var done = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        operationDone = done;
        IsBusy = true; Refresh();
        try { await action(operation.Token); }
        catch (OperationCanceledException) { Status = "Операция Telegram отменена."; }
        catch (Exception ex)
        {
            var message = state.Telegram.Secret.Length == 0 ? ex.Message : ex.Message.Replace(state.Telegram.Secret, "[ключ скрыт]", StringComparison.OrdinalIgnoreCase);
            Status = message;
            log("Telegram: " + message);
        }
        finally
        {
            cancellation = null; IsBusy = false; operationDone = null;
            Refresh(); done.TrySetResult();
        }
    }

    private void Refresh()
    {
        IsRunning = runner.IsRunning;
        Changed(null);
        PrimaryCommand.Refresh(); SaveCommand.Refresh(); UpdateCommand.Refresh(); OpenCommand.Refresh(); CopyCommand.Refresh();
    }

    public async Task ExitAsync()
    {
        exiting = true;
        cancellation?.Cancel();
        if (operationDone != null) await operationDone.Task;
        await runner.StopAsync();
    }

    public void Dispose() { exiting = true; cancellation?.Cancel(); runner.Dispose(); }
}
