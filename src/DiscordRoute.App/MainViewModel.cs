using System.Collections.ObjectModel;
using System.Net.NetworkInformation;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal sealed class MainViewModel : PresentationState, IDisposable
{
    public const string AutomaticStrategy = "Автоматический подбор";
    private readonly string settingsPath = Path.Combine(Program.DataRoot, "settings.json");
    private readonly string logPath = Path.Combine(Program.DataRoot, "app.log");
    private readonly object logLock = new();
    private readonly AppState state;
    private readonly PackageStore packages;
    private readonly WindowsRunner runner;
    private readonly GameIpSet gameIpSet;
    private readonly Action<Action> dispatch;
    private CancellationTokenSource? cancellation;
    private DateTimeOffset nextUpdateAttempt = DateTimeOffset.MinValue;
    private bool ready, stopRequested, exiting, disposed, startupRetry;
    private bool telegramExited;
    public TelegramViewModel Telegram { get; }
    public AppUpdateViewModel AppUpdates { get; }
    private int startupAttempts;
    public event Action<string, string>? Notify;
    public event Action? ExitReady;
    public ObservableCollection<string> Strategies { get; } = [AutomaticStrategy];
    public ObservableCollection<StrategyResult> Results { get; } = [];
    public string SelectedStrategy { get; set; } = AutomaticStrategy;
    public string VoiceAddress { get; set; } = "";
    public string PackageVersion { get; private set => Set(ref field, value); } = "Пакет ещё не загружен";
    public string Journal { get; private set => Set(ref field, value); } = "";
    public bool AutostartEnabled { get; private set => Set(ref field, value); }
    public bool AutoUpdateEnabled { get => state.AutoUpdate; set { state.AutoUpdate = value; Changed(); } }
    public string[] GameFilterModes { get; } = ["Выключен", "TCP и UDP", "Только TCP", "Только UDP"];
    public int GameFilterModeIndex { get; set; }
    public string GameFilterTcpPorts { get; set; } = "1024-65535";
    public string GameFilterUdpPorts { get; set; } = "1024-65535";
    public string GameFilterSummary => "Сохранено: " + GameFilterModes[(int)state.GameFilter.Mode];
    public string GameFilterHint => IsProcessRunning
        ? "Для сохранения сначала отключите подключение на главном экране."
        : "Сохраните параметры, затем подключитесь. IPSet загрузится автоматически.";
    public string GameFilterFeedback { get; private set => Set(ref field, value); } = "";
    public UiCommand ConnectCommand { get; }
    public UiCommand StopCommand { get; }
    public UiCommand PrimaryCommand => ShowDisconnect ? StopCommand : ConnectCommand;
    public UiCommand UpdateCommand { get; }
    public UiCommand AutostartCommand { get; }
    public UiCommand AutoUpdateCommand { get; }
    public UiCommand GameFilterCommand { get; }

    public MainViewModel(Action<Action> dispatch)
    {
        this.dispatch = dispatch;
        state = AppState.Load(settingsPath);
        state.GameFilter = (state.GameFilter ?? throw new InvalidDataException("Повреждены настройки Game Filter.")).Validate();
        GameFilterModeIndex = (int)state.GameFilter.Mode;
        GameFilterTcpPorts = state.GameFilter.TcpPorts;
        GameFilterUdpPorts = state.GameFilter.UdpPorts;
        packages = new PackageStore(Path.Combine(Program.DataRoot, "Packages"), Log);
        gameIpSet = new GameIpSet(Path.Combine(Program.DataRoot, "GameFilter", "ipset-all.txt"), Log);
        runner = new WindowsRunner(Log, () => state.GameFilter, gameIpSet.FilePath);
        ConnectCommand = new(() => RunAsync(ConnectAsync), () => !IsBusy && !exiting);
        UpdateCommand = new(() => RunAsync(ct => UpdateAsync(true, ct)), () => !IsBusy && !exiting);
        StopCommand = new(StopAsync, () => !exiting);
        AutostartCommand = new(() => RunAsync(async _ =>
        {
            try { await Autostart.SetAsync(!AutostartEnabled); state.AutostartConfigured = true; Save(); }
            finally { AutostartEnabled = await Autostart.IsEnabledAsync(); Changed(nameof(AutostartEnabled)); }
            Log(AutostartEnabled ? "Автозапуск включён: через 15 секунд после входа в Windows." : "Автозапуск выключен.");
        }), () => !IsBusy && ready && !exiting);
        AutoUpdateCommand = new(() => RunAsync(_ => { Save(); AppUpdates.SettingsChanged(); Log(state.AutoUpdate ? "Автообновление включено." : "Автообновление выключено."); return Task.CompletedTask; }), () => !IsBusy && !exiting);
        GameFilterCommand = new(ApplyGameFilterAsync, () => !IsBusy && !runner.IsRunning && !exiting);
        Telegram = new(state, Save, Log);
        AppUpdates = new(state, Log, app =>
        {
            var arguments = new List<string>();
            if (runner.IsRunning) arguments.Add("--resume-discord");
            if (Telegram.IsRunning) arguments.Add("--resume-telegram");
            Program.Restart = new(app, arguments.ToArray());
            RequestExit();
        });
    }

    private async Task ApplyGameFilterAsync()
    {
        if (IsBusy || runner.IsRunning || exiting) return;
        GameFilterSettings next;
        try
        {
            next = new GameFilterSettings { Mode = (GameFilterMode)GameFilterModeIndex,
                TcpPorts = GameFilterTcpPorts, UdpPorts = GameFilterUdpPorts }.Validate();
        }
        catch (InvalidDataException ex) { GameFilterFeedback = ex.Message; return; }
        await RunAsync(_ =>
        {
            var previous = state.GameFilter;
            try { state.GameFilter = next; Save(); }
            catch (Exception ex) { state.GameFilter = previous; GameFilterFeedback = "Не сохранено: " + ex.Message; throw; }
            GameFilterTcpPorts = next.TcpPorts;
            GameFilterUdpPorts = next.UdpPorts;
            GameFilterFeedback = "Параметры сохранены и применятся при следующем подключении.";
            Changed(null);
            Log("Game Filter: " + GameFilterModes[(int)next.Mode] + "; TCP " + next.TcpPorts + "; UDP " + next.UdpPorts);
            return Task.CompletedTask;
        });
    }

    public async Task InitializeAsync(bool startup, bool resumeDiscord = false, bool resumeTelegram = false)
    {
        var telegramInitialization = Telegram.InitializeAsync(resumeTelegram);
        var appUpdate = AppUpdates.TickAsync();
        startupRetry = startup || resumeDiscord;
        await RunAsync(async ct =>
        {
            if (!state.AutostartConfigured)
            {
                try { await Autostart.SetAsync(true); state.AutostartConfigured = true; Save(); Log("Автозапуск включён."); }
                catch (Exception ex) { Log("Автозапуск не настроен: " + ex.Message); }
            }
            AutostartEnabled = await Autostart.IsEnabledAsync();
            if (AutostartEnabled)
            {
                try { await Autostart.SetAsync(true); }
                catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
                { Log("Копия для автозапуска не обновилась: " + ex.Message); SetStatus("Не удалось обновить установленную копию. Подробности — в журнале.", true); }
            }
            ready = true;
            RefreshPackage();
            if (startup || resumeDiscord) { startupAttempts++; await ConnectAsync(ct); }
            await UpdateAsync(false, ct);
        });
        await Task.WhenAll(telegramInitialization, appUpdate);
    }

    public Task TickAsync() => Task.WhenAll(TickDiscordAsync(), Telegram.TickAsync(), AppUpdates.TickAsync());

    private async Task TickDiscordAsync()
    {
        if (IsBusy || exiting) return;
        if (startupRetry && startupAttempts < 5 && NetworkInterface.GetIsNetworkAvailable())
        {
            startupAttempts++;
            Log("Повторное подключение после запуска Windows, попытка " + startupAttempts);
            await RunAsync(ConnectAsync);
            return;
        }
        if (IsProcessRunning && !runner.IsRunning)
        { IsVerified = false; SetStatus("Обход остановился. Нажмите «Подобрать и подключить».", true); }
        if (state.AutoUpdate && DateTimeOffset.UtcNow >= nextUpdateAttempt)
            await RunAsync(ct => UpdateAsync(false, ct));
    }

    public async void RequestExit()
    {
        if (exiting) return;
        exiting = true;
        cancellation?.Cancel();
        try { await Task.WhenAll(Telegram.ExitAsync(), AppUpdates.ExitAsync()); }
        catch (Exception ex) { Log("Ошибка остановки Telegram: " + ex.Message); Telegram.Dispose(); }
        finally { telegramExited = true; CompleteExit(); }
    }

    private void CompleteExit() { if (exiting && telegramExited && cancellation == null) ExitReady?.Invoke(); }

    private async Task UpdateAsync(bool force, CancellationToken ct)
    {
        if (!force && (!state.AutoUpdate || DateTimeOffset.UtcNow < nextUpdateAttempt)) return;
        nextUpdateAttempt = DateTimeOffset.UtcNow.AddHours(6);
        var active = packages.Open(state.ActivePackage);
        var pending = packages.Open(state.PendingPackage);
        Log("Проверка обновлений GitHub…");
        try
        {
            var newer = await packages.StageLatestAsync(pending?.Version ?? active?.Version, active?.Root, ct);
            state.LastUpdateCheck = DateTimeOffset.UtcNow;
            if (newer == null) { Save(); Log("Установлен актуальный пакет."); return; }
            state.PendingPackage = newer.Id;
            Save();
            RefreshPackage();
            Log("Новый пакет подготовлен. Он станет активным только после успешных проверок.");
            if (runner.IsRunning) await ConnectAsync(ct);
        }
        catch (Exception ex) when (ex is HttpRequestException or IOException or System.Text.Json.JsonException)
        {
            Log("Обновление не применено: " + ex.Message);
            if (active == null && pending == null) throw;
        }
    }

    private async Task ConnectAsync(CancellationToken ct)
    {
        if (!NetworkInterface.GetIsNetworkAvailable()) throw new IOException("Нет активного сетевого подключения.");
        if (!runner.IsRunning) WindowsRunner.EnsureNoExternalProcess();
        var manual = SelectedStrategy == AutomaticStrategy ? null : SelectedStrategy;
        if (state.ActivePackage == null && state.PendingPackage == null) await UpdateAsync(true, ct);
        var active = packages.Open(state.ActivePackage);
        var pending = packages.Open(state.PendingPackage);
        var package = pending ?? active ?? throw new IOException("Сначала нужно загрузить пакет zapret с GitHub.");
        if (state.GameFilter.Mode != GameFilterMode.Disabled) await gameIpSet.EnsureAsync(ct);
        if (pending != null && active != null) PackageStore.PrepareLists(pending.Root, active.Root);
        var endpoint = GetVoiceEndpoint();
        if (endpoint == null && state.VoiceServer != null && state.VoiceObservedAt > DateTimeOffset.UtcNow.AddDays(-7))
            endpoint = VoiceEndpoint.Parse(state.VoiceServer);
        if (endpoint == null)
        {
            SetStatus("Откройте голосовой канал Discord: ждём адрес сервера…");
            Log("Нет адреса голосового сервера. Откройте Discord и попробуйте подключиться к голосовому каналу в течение минуты. Подтверждать звук не нужно.");
            var bootstrap = !runner.IsRunning;
            try
            {
                if (bootstrap)
                {
                    var usable = false;
                    foreach (var item in Strategy.Load(package.Root, Log).OrderBy(s => s.Name == state.LastStrategy ? 0 : 1))
                    {
                        ct.ThrowIfCancellationRequested();
                        try { await runner.StartAsync(package, item, ct); }
                        catch (Exception ex) when (ex is IOException or System.ComponentModel.Win32Exception) { Log(ex.Message); continue; }
                        var control = await DiscordProbe.RunControlAsync(ct);
                        Log(item.Name + " (получение адреса): API " + control.Api.Detail + "; Gateway " + control.Gateway.Detail);
                        if (!control.Api.Success || !control.Gateway.Success) continue;
                        usable = true;
                        break;
                    }
                    if (!usable) throw new IOException("Ни одна стратегия не открыла API и Gateway Discord для получения голосового адреса.");
                    Log("Временная стратегия открыла API и Gateway. Теперь подключитесь к голосовому каналу; голос ещё не проверен.");
                }
                for (var attempt = 0; attempt < 20 && endpoint == null; attempt++)
                {
                    await Task.Delay(3000, ct);
                    endpoint = GetVoiceEndpoint();
                }
            }
            finally { if (bootstrap) await runner.StopAsync(); }
            if (endpoint == null) throw new IOException("Адрес не найден в журналах Discord. Попробуйте подключиться к каналу в настольном Discord или укажите адрес *.discord.media:порт в поле выше. Проверка голоса не выполнена.");
        }
        Log("Проверяем голосовой сервер: " + endpoint.Authority);
        IsVerified = false;
        Results.Clear();
        SetStatus("Проверяем стратегии. Это может занять несколько минут…");
        var session = new ConnectionSession(runner, state, Save, Report, Log);
        var selected = await session.ConnectAsync(active, pending, new DiscordProbe(endpoint).RunAsync, manual, ct);
        IsVerified = true;
        startupRetry = false;
        SetStatus(selected.Name + " · голосовой сервер отвечает");
        Log("Две проверки API, Gateway и голосового сервера прошли. Оставлена стратегия " + selected.Name);
        RefreshPackage();
        Notify?.Invoke("Связь: подключено", selected.Name + ": голосовой сервер отвечает");
    }

    private async Task RunAsync(Func<CancellationToken, Task> action)
    {
        if (cancellation != null || exiting) return;
        using var operation = new CancellationTokenSource();
        cancellation = operation;
        stopRequested = false;
        SetBusy(true);
        try { await action(operation.Token); }
        catch (OperationCanceledException) { Log("Операция отменена."); }
        catch (Exception ex)
        {
            IsVerified = false;
            Log(ex.Message);
            SetStatus(runner.IsRunning ? "Предыдущий процесс работает; проверки не пройдены." : "Не подключено. Подробности — в журнале.", true);
            Notify?.Invoke("Связь: требуется внимание", ex.Message[..Math.Min(ex.Message.Length, 220)]);
        }
        finally
        {
            if (stopRequested || exiting)
            {
                try { await runner.StopAsync(); } catch (Exception ex) { Log("Ошибка остановки: " + ex.Message); }
                IsVerified = false;
                SetStatus(runner.IsRunning ? "Не удалось остановить процесс." : "Отключено", runner.IsRunning);
            }
            cancellation = null;
            SetBusy(false);
            CompleteExit();
        }
    }

    private async Task StopAsync()
    {
        startupRetry = false;
        if (cancellation != null) { stopRequested = true; cancellation.Cancel(); return; }
        await RunAsync(async _ =>
        {
            await runner.StopAsync();
            IsVerified = false;
            ShowServiceResults(null);
            SetStatus("Отключено");
            Log("Обход остановлен.");
        });
    }

    private void Report(Strategy item, ProbeResult? probe, string outcome)
    {
        ShowServiceResults(probe, checking: probe == null && !outcome.StartsWith("Ошибка", StringComparison.Ordinal));
        var row = new StrategyResult(item.Name, probe?.Api.Detail ?? "—", probe?.Gateway.Detail ?? "—", probe?.Voice.Detail ?? "—", outcome);
        var existing = Results.FirstOrDefault(r => r.Name == item.Name);
        if (existing == null) Results.Add(row); else Results[Results.IndexOf(existing)] = row;
        Log(item.Name + ": " + outcome + (probe == null ? "" : " | API: " + probe.Api.Detail + " | Gateway: " + probe.Gateway.Detail + " | Голос: " + probe.Voice.Detail));
    }

    private void RefreshPackage()
    {
        var active = packages.Open(state.ActivePackage);
        var pending = packages.Open(state.PendingPackage);
        PackageVersion = "zapret " + (active?.Version ?? pending?.Version ?? "не загружен") +
            (pending == null ? "" : " · ожидает проверки" );
        var current = SelectedStrategy;
        Strategies.Clear(); Strategies.Add(AutomaticStrategy);
        if ((pending ?? active) is Package available)
            foreach (var item in Strategy.Load(available.Root, Log)) Strategies.Add(item.Name);
        SelectedStrategy = Strategies.Contains(current) ? current : AutomaticStrategy;
        Changed(nameof(SelectedStrategy));
    }

    private void Save() => state.Save(settingsPath);
    private Uri? GetVoiceEndpoint()
    {
        if (VoiceAddress.Trim().Length > 0)
        {
            var endpoint = VoiceEndpoint.Parse(VoiceAddress);
            state.VoiceServer = endpoint.Authority;
            state.VoiceObservedAt = DateTimeOffset.UtcNow;
            state.VoiceLogFingerprint = null;
            return endpoint;
        }
        if (VoiceDiscovery.Find(Log) is not { } found) return null;
        if (found.Fingerprint != state.VoiceLogFingerprint)
        {
            state.VoiceServer = found.Endpoint.Authority;
            state.VoiceObservedAt = found.ObservedAt;
            state.VoiceLogFingerprint = found.Fingerprint;
        }
        return state.VoiceObservedAt > DateTimeOffset.UtcNow.AddDays(-7) ? found.Endpoint : null;
    }
    private void SetBusy(bool busy)
    {
        IsBusy = busy;
        IsProcessRunning = runner.IsRunning;
        if (!busy && (stopRequested || Api.Kind == CheckKind.Checking)) ShowServiceResults(null);
        ConnectCommand.Refresh(); UpdateCommand.Refresh(); StopCommand.Refresh(); AutostartCommand.Refresh(); AutoUpdateCommand.Refresh(); GameFilterCommand.Refresh();
    }

    private void SetStatus(string text, bool error = false)
    {
        Status = text;
        HasError = error;
        IsProcessRunning = runner.IsRunning;
        GameFilterCommand.Refresh();
        if (error && !IsVerified) ShowServiceResults(null);
    }

    private void Log(string message)
    {
        var line = DateTimeOffset.Now.ToString("yyyy-MM-dd HH:mm:ss") + "  " + message + Environment.NewLine;
        lock (logLock)
        {
            try
            {
                if (File.Exists(logPath) && new FileInfo(logPath).Length > 1024 * 1024) File.Move(logPath, logPath + ".1", true);
                File.AppendAllText(logPath, line);
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException) { }
        }
        dispatch(() =>
        {
            if (disposed) return;
            if (Journal.Length > 40_000) Journal = Journal[^20_000..];
            Journal += line;
        });
    }

    public void Dispose()
    {
        disposed = true;
        cancellation?.Cancel();
        runner.Dispose();
        Telegram.Dispose();
        AppUpdates.Dispose();
    }
}
