using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Windows.Input;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal enum CheckKind { Idle, Checking, Passed, Failed, Unconfirmed }
internal sealed record ServiceCheck(string Name, string Description, CheckKind Kind, string Detail)
{
    public string Label => Kind switch { CheckKind.Passed => "Отвечает", CheckKind.Failed => "Не отвечает", CheckKind.Unconfirmed => "Без подтверждения", CheckKind.Checking => "Проверяем…", _ => "Не проверено" };
}
internal sealed record StrategyResult(string Name, string Api, string Gateway, string Voice, string YouTube, string Outcome, string Quality = "Замеров пока нет");

internal class ObservableState : INotifyPropertyChanged
{
    public event PropertyChangedEventHandler? PropertyChanged;
    protected void Changed([CallerMemberName] string? name = null) => PropertyChanged?.Invoke(this, new(name));
    protected void Set<T>(ref T storage, T value, [CallerMemberName] string? name = null)
    {
        if (EqualityComparer<T>.Default.Equals(storage, value)) return;
        storage = value;
        Changed(name);
    }
}

internal class PresentationState : ObservableState
{
    private void ChangeState(ref bool storage, bool value)
    {
        if (storage == value) return;
        storage = value;
        Changed(null);
    }
    public bool IsBusy { get; set => ChangeState(ref field, value); }
    public bool IsVerified { get; set => ChangeState(ref field, value); }
    public bool IsProcessRunning { get; set => ChangeState(ref field, value); }
    public bool HasError { get; set => ChangeState(ref field, value); }
    public bool CanEdit => !IsBusy;
    public bool ShowDisconnect => IsBusy || IsProcessRunning;
    public string PrimaryActionLabel => IsBusy ? "Отменить и отключить" : IsProcessRunning ? "Отключить" : "Подключить";
    public string StopLabel => IsBusy ? "Отменить и отключить" : "Отключить";
    public string Heading => IsBusy ? "Проверяем подключение" : IsVerified ? "Discord и YouTube\nотвечают" : HasError ? "Нужно ваше внимание" : IsProcessRunning ? "Процесс запущен" : "Связь выключена";
    public string Badge => IsBusy ? "ВЫПОЛНЯЕТСЯ ОПЕРАЦИЯ" : IsVerified ? "ПРОВЕРКИ ПРОЙДЕНЫ" : IsProcessRunning ? "НЕ ПРОВЕРЕНО" : "НЕ ПОДКЛЮЧЕНО";
    public string Status { get; set => Set(ref field, value); } = "При подключении проверим выбранные сервисы.";
    public ServiceCheck Api { get; private set => Set(ref field, value); } = Make("API", "Доступ к Discord", null, false);
    public ServiceCheck Gateway { get; private set => Set(ref field, value); } = Make("Gateway", "Соединение с Discord", null, false);
    public ServiceCheck Voice { get; private set => Set(ref field, value); } = Make("Голосовой сервер", "Ответ сервера канала", null, false);
    public ServiceCheck YouTube { get; private set => Set(ref field, value); } = Make("YouTube · HTTPS", "Сайт и Googlevideo; воспроизведение видео не проверяется", null, false);
    public void ShowServiceResults(ProbeResult? result, bool checking = false)
    {
        Api = Make("API", "Доступ к Discord", result?.Api, checking);
        Gateway = Make("Gateway", "Соединение с Discord", result?.Gateway, checking);
        Voice = Make("Голосовой сервер", "Ответ сервера канала", result?.Voice, checking);
        YouTube = Make("YouTube · HTTPS", "Сайт и Googlevideo", result == null ? null :
            new(result.YouTube.Success, result.YouTube.Detail + ". Воспроизведение видео не проверяется.", result.YouTube.Site.Skipped), checking);
    }
    private static ServiceCheck Make(string name, string description, CheckResult? result, bool checking) =>
        new(name, description, result?.Skipped == true ? CheckKind.Idle : result == null ? checking ? CheckKind.Checking : CheckKind.Idle : result.Success ? CheckKind.Passed : CheckKind.Failed,
            result?.Detail ?? (checking ? "Проверяем соединение" : "Результат появится после проверки"));
}

internal sealed class UiCommand(Func<Task> execute, Func<bool> canExecute) : ICommand
{
    public event EventHandler? CanExecuteChanged;
    public bool CanExecute(object? parameter) => canExecute();
    public async void Execute(object? parameter) { if (CanExecute(parameter)) await execute(); }
    public void Refresh() => CanExecuteChanged?.Invoke(this, EventArgs.Empty);
}
