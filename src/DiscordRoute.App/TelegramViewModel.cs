using System.Diagnostics;
using System.Globalization;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal sealed class TelegramViewModel : ObservableState, IDisposable
{
    private readonly AppState state;
    private readonly Action save;
    private readonly Action<string> log;
    private readonly TelegramRunner runner;
    private CancellationTokenSource? cancellation;
    private TaskCompletionSource? operationDone;
    private bool exiting;
    private int environmentVersion;
    private DateTimeOffset? startedAt;
    private DateTimeOffset nextProbe;
    internal ConnectionRecovery Recovery { get; } = new();
    public event Action<string, string>? Notify;
    public event Action? SettingsRequested;
    public bool ServerVerified { get; private set; }
    public ServiceCheck ServerCheck { get; private set; } = new("Сервер Telegram", "Ответ MTProto через прокси", CheckKind.Idle, "Ещё не проверен");
    public bool HasFailure { get; private set; }
    private ConnectionHelp? help;
    public string HelpActionLabel => help?.ActionLabel ?? "Повторить проверку";
    public UiCommand CheckCommand { get; }
    public UiCommand HelpCommand { get; }

    public string Summary => IsBusy ? "Telegram · операция…" : ServerVerified ? "Telegram · сервер отвечает" : IsRunning ? "Telegram · не проверен" : "Telegram · выключен";
    public string ConnectionTime => startedAt is { } at && IsRunning ? "Прокси работает " + Math.Max(0, (int)(DateTimeOffset.UtcNow - at).TotalMinutes) + " мин" : "Прокси не запущен";
    public int? LastError { get; private set; }
    public string Port { get; set; }
    public bool StartWithApp { get; set; }
    public bool IsBusy { get; private set; }
    public bool IsRunning { get; private set; }
    public bool CanEdit => !IsBusy && !IsRunning;
    public string PrimaryLabel => IsBusy ? "Отменить" : IsRunning ? "Остановить прокси" : Recovery.Wanted ? "Отменить повторы" : "Запустить прокси";
    public string Status { get; private set => Set(ref field, value); } = "Прокси работает внутри «Связи». Подключение Discord включается независимо.";
    public string Version => "Встроенный движок Flowseal " + TelegramRuntime.Version;
    public string Endpoint => "127.0.0.1:" + state.Telegram.Port;
    public UiCommand PrimaryCommand { get; }
    public UiCommand SaveCommand { get; }
    public UiCommand OpenCommand { get; }
    public UiCommand CopyCommand { get; }

    public TelegramViewModel(AppState state, Action save, Action<string> log)
    {
        this.state = state; this.save = save; this.log = log;
        runner = new();
        state.Telegram = (state.Telegram ?? throw new InvalidDataException("Повреждены настройки Telegram.")).Validate();
        Port = state.Telegram.Port.ToString(CultureInfo.InvariantCulture);
        StartWithApp = state.Telegram.StartWithApp;
        PrimaryCommand = new(ToggleAsync, () => !exiting);
        CheckCommand = new(() => RunAsync(CheckServerAsync), () => IsRunning && !IsBusy && !exiting);
        HelpCommand = new(ResolveFailureAsync, () => !IsBusy && !exiting);
        SaveCommand = new(() => RunAsync(_ => { SaveSettings(); Status = "Настройки Telegram сохранены."; return Task.CompletedTask; }), () => CanEdit && !exiting);
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

    public Task InitializeAsync(bool resume = false) => state.Telegram.StartWithApp || resume ? StartRequestedAsync() : Task.CompletedTask;

    public void NetworkChanged(DateTimeOffset now)
    {
        environmentVersion++;
        Recovery.Invalidate(now);
        ServerVerified = false;
        ServerCheck = new("Сервер Telegram", "Ответ MTProto через прокси", CheckKind.Idle, "Сеть изменилась; ожидаем повторную проверку");
        Changed(null);
    }

    public async Task TickAsync(bool notifications = true)
    {
        Changed(nameof(ConnectionTime));
        if (IsBusy || exiting) return;
        var now = DateTimeOffset.UtcNow;
        if (IsRunning && !runner.IsRunning)
        {
            ServerVerified = false;
            Status = "Telegram-прокси остановился. Попробуем восстановить подключение.";
            Recovery.Failed(now);
            Refresh();
        }
        if (Recovery.ShouldNotify(now, notifications)) Notify?.Invoke("Telegram: соединение не восстановлено", "Откройте «Связь»: там показана причина и действие для восстановления.");
        if (Recovery.TryBegin(now, System.Net.NetworkInformation.NetworkInterface.GetIsNetworkAvailable()))
        {
            await RunAsync(async ct =>
            {
                var version = environmentVersion;
                if (runner.IsRunning) await CheckServerAsync(ct);
                if (version != environmentVersion) return;
                if (!ServerVerified)
                {
                    await runner.StopAsync();
                    await StartAsync(ct);
                }
            });
        }
        else if (IsRunning && !Recovery.Pending && now >= nextProbe) await RunAsync(CheckServerAsync);
    }

    public Task ResolveFailureAsync()
    {
        if (help?.OpenSettings == true) { SettingsRequested?.Invoke(); return Task.CompletedTask; }
        return StartRequestedAsync();
    }

    public Task StartRequestedAsync()
    {
        if (IsBusy || exiting) return Task.CompletedTask;
        Recovery.Enable();
        return RunAsync(async ct => { if (runner.IsRunning) await CheckServerAsync(ct); else await StartAsync(ct); });
    }

    public async Task StopRequestedAsync()
    {
        Recovery.Stop();
        cancellation?.Cancel();
        if (operationDone != null) await operationDone.Task;
        await RunAsync(async _ =>
        {
            await runner.StopAsync();
            ServerVerified = false;
            HasFailure = false;
            ServerCheck = new("Сервер Telegram", "Ответ MTProto через прокси", CheckKind.Idle, "Прокси отключён");
            Status = "Прокси остановлен. При необходимости выключите его в настройках Telegram.";
            log("Telegram: прокси остановлен.");
        });
    }

    private Task ToggleAsync() => IsBusy || runner.IsRunning || Recovery.Wanted ? StopRequestedAsync() : StartRequestedAsync();

    private async Task CheckServerAsync(CancellationToken ct)
    {
        ServerVerified = false;
        ServerCheck = new("Сервер Telegram", "Ответ MTProto через прокси", CheckKind.Checking, "Проверяем ответ сервера…");
        Changed(null);
        var version = environmentVersion;
        var passed = await runner.ProbeAsync(ct);
        if (version != environmentVersion) return;
        nextProbe = DateTimeOffset.UtcNow.AddMinutes(1);
        ServerVerified = passed;
        HasFailure = !passed;
        ServerCheck = new("Сервер Telegram", "Ответ MTProto через прокси", passed ? CheckKind.Passed : CheckKind.Failed,
            passed ? "Получен ответ MTProto. Сообщения и звонки не проверялись." : "Нет корректного ответа за отведённое время.");
        if (passed)
        {
            Recovery.Succeeded(); help = null;
            Status = "Прокси запущен, сервер Telegram отвечает. Подключите его в Telegram.";
        }
        else
        {
            Recovery.Failed(DateTimeOffset.UtcNow);
            help = new("Локальный прокси запущен, но сервер Telegram не ответил. Проверьте интернет и повторите проверку.");
            Status = help.Message;
        }
        Changed(null);
    }

    private void SaveSettings()
    {
        if (!int.TryParse(Port, NumberStyles.None, CultureInfo.InvariantCulture, out var port))
            throw new ConnectionFailure("Укажите целый номер порта от 1 до 65535 в настройках Telegram.", true);
        var previous = state.Telegram;
        if (port is < 1 or > 65535) throw new ConnectionFailure("Порт Telegram должен быть от 1 до 65535.", true);
        var next = (previous with { Port = port, StartWithApp = StartWithApp }).WithSecret();
        try { state.Telegram = next; save(); }
        catch { state.Telegram = previous; throw; }
        Changed(null);
    }

    private async Task StartAsync(CancellationToken ct)
    {
        SaveSettings();
        var configuration = state.Telegram.EmbeddedConfiguration(Path.Combine(Program.DataRoot, "Telegram"),
            state.TelegramActivePackage, state.TelegramPendingPackage);
        Status = "Запускаем встроенный прокси…";
        await runner.StartAsync(configuration, state.Telegram, ct);
        startedAt = DateTimeOffset.UtcNow;
        Status = "Встроенный прокси готов. Нажмите «Открыть в Telegram» и подтвердите подключение.";
        log("Telegram: встроенный движок запущен; проверяем сервер.");
        await CheckServerAsync(ct);
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
        catch (OperationCanceledException) { ServerVerified = false; ServerCheck = new("Сервер Telegram", "Ответ MTProto через прокси", CheckKind.Idle, "Проверка отменена"); Status = "Операция Telegram отменена."; }
        catch (Exception ex)
        {
            LastError = ex.HResult;
            var message = state.Telegram.Secret.Length == 0 ? ex.Message : ex.Message.Replace(state.Telegram.Secret, "[ключ скрыт]", StringComparison.OrdinalIgnoreCase);
            help = ConnectionHelp.Describe(ex, System.Net.NetworkInformation.NetworkInterface.GetIsNetworkAvailable());
            Status = help.Message;
            HasFailure = true; ServerVerified = false;
            Recovery.Failed(DateTimeOffset.UtcNow);
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
        if (!IsRunning) { startedAt = null; ServerVerified = false; }
        Changed(null);
        PrimaryCommand.Refresh(); SaveCommand.Refresh(); OpenCommand.Refresh(); CopyCommand.Refresh(); CheckCommand.Refresh(); HelpCommand.Refresh();
    }

    public async Task ExitAsync()
    {
        exiting = true;
        Recovery.Stop();
        cancellation?.Cancel();
        if (operationDone != null) await operationDone.Task;
        await runner.StopAsync();
    }

    public void Dispose() { exiting = true; cancellation?.Cancel(); runner.Dispose(); }
}
