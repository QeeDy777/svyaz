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
    private DateTimeOffset? startedAt;
    public string Summary => IsBusy ? "Telegram · операция…" : IsRunning ? "Telegram · прокси запущен" : "Telegram · выключен";
    public string ConnectionTime => startedAt is { } at && IsRunning ? "Прокси работает " + Math.Max(0, (int)(DateTimeOffset.UtcNow - at).TotalMinutes) + " мин" : "Прокси не запущен";
    public int? LastError { get; private set; }
    public string Port { get; set; }
    public bool StartWithApp { get; set; }
    public bool IsBusy { get; private set; }
    public bool IsRunning { get; private set; }
    public bool CanEdit => !IsBusy && !IsRunning;
    public string PrimaryLabel => IsBusy ? "Отменить" : IsRunning ? "Остановить прокси" : "Запустить прокси";
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

    public Task InitializeAsync(bool resume = false) => RunAsync(async ct =>
    {
        if (state.Telegram.StartWithApp || resume) await StartAsync(ct);
    });

    public Task TickAsync()
    {
        Changed(nameof(ConnectionTime));
        if (!IsBusy && !exiting && IsRunning && !runner.IsRunning)
        {
            Status = "Telegram-прокси остановился. Запустите его снова или отключите прокси в Telegram.";
            Refresh();
        }
        return Task.CompletedTask;
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
        var configuration = state.Telegram.EmbeddedConfiguration(Path.Combine(Program.DataRoot, "Telegram"),
            state.TelegramActivePackage, state.TelegramPendingPackage);
        Status = "Запускаем встроенный прокси…";
        await runner.StartAsync(configuration, state.Telegram, ct);
        startedAt = DateTimeOffset.UtcNow;
        Status = "Встроенный прокси готов. Нажмите «Открыть в Telegram» и подтвердите подключение.";
        log("Telegram: встроенный движок запущен; локальный порт отвечает. Сообщения и звонки не проверялись.");
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
            LastError = ex.HResult;
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
        if (!IsRunning) startedAt = null;
        Changed(null);
        PrimaryCommand.Refresh(); SaveCommand.Refresh(); OpenCommand.Refresh(); CopyCommand.Refresh();
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
