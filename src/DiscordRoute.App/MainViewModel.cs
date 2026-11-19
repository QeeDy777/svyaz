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
    private readonly string preferencesPath = Path.Combine(Program.DataRoot, "preferences.json");
    private readonly ConvenienceState preferences;
    private readonly PackageStore packages;
    private readonly WindowsRunner runner;
    private readonly GameIpSet gameIpSet;
    private readonly Action<Action> dispatch;
    private CancellationTokenSource? cancellation;
    private DateTimeOffset nextUpdateAttempt = DateTimeOffset.MinValue;
    private bool ready, stopRequested, exiting, disposed;
    private bool telegramExited, ticking;
    private bool backgroundCheck;
    private int environmentVersion;
    private readonly ConnectionRecovery recovery = new();
    private readonly ResponseHistory quality = new();
    private int qualityEnvironmentVersion;
    public string QualitySummary => quality.Quality.Summary;
    public bool HasQuality => quality.Quality.Total > 0;
    private ConnectionHelp? help;
    public event Action? SettingsRequested;
    public ServiceSelection Services => preferences.Services!;
    public bool UseDiscord { get; set; }
    public bool UseYouTube { get; set; }
    public bool UseTelegram { get; set; }
    public bool QuietNotifications { get; set; }
    public bool CanConfigureServices => !IsBusy && !runner.IsRunning && !Telegram.IsBusy && !Telegram.IsRunning && !recovery.Wanted && !Telegram.Recovery.Wanted;
    public bool AnyBusy => IsBusy || Telegram.IsBusy;
    public bool ShowProgress => IsBusy && !backgroundCheck || Telegram.ShowProgress;
    public new bool ShowDisconnect => AnyBusy || IsProcessRunning || Telegram.IsRunning || recovery.Wanted || Telegram.Recovery.Wanted;
    public new string PrimaryActionLabel => ShowProgress ? "Отменить и отключить" : ShowDisconnect ? "Отключить" : "Подключить";
    public new string Heading => ShowProgress ? "Проверяем подключение" : HasError || Services.Telegram && Telegram.HasFailure ? "Нужно ваше внимание" :
        IsProcessRunning || Telegram.IsRunning ? "Связь включена" :
        ShowDisconnect ? "Ожидаем подключения" : "Связь выключена";
    public string SelectedServicesSummary => string.Join(" · ", new[] { Services.Discord ? "Discord" : null, Services.YouTube ? "YouTube" : null, Services.Telegram ? "Telegram" : null }.OfType<string>());
    public string SelectionFeedback { get; private set => Set(ref field, value); } = "";
    public string ConnectionStatus => Services.Telegram && (!Services.UsesZapret || Telegram.HasFailure) ? Telegram.Status : Status;
    public string HelpActionLabel => Services.Telegram && Telegram.HasFailure ? Telegram.HelpActionLabel : help?.ActionLabel ?? "Повторить проверку";
    public UiCommand ServicesCommand { get; }
    public UiCommand HelpCommand { get; }

    public TelegramViewModel Telegram { get; }
    public AppUpdateViewModel AppUpdates { get; }
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
    private DateTimeOffset? lastSuccessfulCheck;
    private string? networkId;
    public bool ShowRepair => IsProcessRunning || HasError || Services.Telegram && Telegram.HasFailure;
    public bool HasLastCheck => lastSuccessfulCheck != null;
    public string CurrentStrategy => IsProcessRunning ? runner.StrategyName ?? "Подбор…" : "—";
    public string DiscordSummary => !Services.Discord ? "Discord · не выбран" : IsBusy && !backgroundCheck ? "Discord · проверка…" : IsVerified ? "Discord · сервер отвечает" : IsProcessRunning ? "Discord · не проверен" : "Discord · выключен";
    public string ConnectionTime => runner.StartedAt is { } at && IsProcessRunning ? "Подключено " + Math.Max(0, (int)(DateTimeOffset.UtcNow - at).TotalMinutes) + " мин" : "Подключение не запущено";
    public string LastCheck => lastSuccessfulCheck is { } at ? "Успешная проверка · " + at.ToLocalTime().ToString("dd.MM HH:mm") : "Успешных проверок пока нет";
    public string NetworkSummary => networkId == null ? "Сеть не определена · обычный подбор" : preferences.NetworkStrategies.TryGetValue(networkId, out var strategy) ? "Для этой сети сохранён " + strategy : "Запомним альт после успешной проверки";
    private int? lastError;
    private ProbeResult? lastProbe;
    public string DiagnosticFeedback { get; private set => Set(ref field, value); } = "Версия, результаты проверок и коды ошибок. Без ключей, адресов и исходных журналов.";
    public UiCommand ExportDiagnosticsCommand { get; }
    public UiCommand RepairCommand { get; }
    public UiCommand FullScanCommand { get; }
    public UiCommand ConnectCommand { get; }
    public UiCommand StopCommand { get; }
    public UiCommand PrimaryCommand => ShowDisconnect ? StopCommand : ConnectCommand;
    public UiCommand ResetZapretCommand { get; }
    public string ResetFeedback { get; private set => Set(ref field, value); } = "";
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
        preferences = ConvenienceState.Load(preferencesPath);
        preferences.Services ??= new() { Telegram = state.Telegram.StartWithApp };
        preferences.Services.Validate();
        UseDiscord = Services.Discord; UseYouTube = Services.YouTube; UseTelegram = Services.Telegram;
        QuietNotifications = preferences.QuietNotifications;
        networkId = CurrentNetwork.Read();
        ServicesCommand = new(SaveServicesAsync, () => CanConfigureServices && !exiting);
        ExportDiagnosticsCommand = new(ExportDiagnosticsAsync, () => !exiting);
        RepairCommand = new(() => ConnectSelectedAsync(true), () => !AnyBusy && !exiting);
        FullScanCommand = new(() => ConnectSelectedAsync(fullScan: true), () => Services.UsesZapret && !AnyBusy && !exiting);
        ConnectCommand = new(() => ConnectSelectedAsync(), () => !AnyBusy && !exiting);
        UpdateCommand = new(() => RunAsync(ct => UpdateAsync(true, ct)), () => !IsBusy && !exiting);
        ResetZapretCommand = new(ResetZapretAsync, () => ready && !IsBusy && !exiting);
        StopCommand = new(StopAsync, () => !exiting);
        AutostartCommand = new(() => RunAsync(async _ =>
        {
            try { await Autostart.SetAsync(!AutostartEnabled); state.AutostartConfigured = true; Save(); }
            finally { AutostartEnabled = await Autostart.IsEnabledAsync(); Changed(nameof(AutostartEnabled)); }
            Log(AutostartEnabled ? "Автозапуск включён: через 15 секунд после входа в Windows." : "Автозапуск выключен.");
        }), () => !IsBusy && ready && !exiting);
        GameFilterCommand = new(ApplyGameFilterAsync, () => !IsBusy && !runner.IsRunning && !exiting);
        Telegram = new(state, Save, Log);
        HelpCommand = new(() => { if (Services.Telegram && Telegram.HasFailure) return Telegram.ResolveFailureAsync(); if (help?.OpenSettings == true) { SettingsRequested?.Invoke(); return Task.CompletedTask; } return ConnectSelectedAsync(true); }, () => !AnyBusy && !exiting);
        Telegram.Notify += (title, text) => Notify?.Invoke(title, text);
        Telegram.PropertyChanged += (_, _) => { Changed(null); RefreshCommands(); };
        AppUpdates = new(state, Log, app => RestartApp(app, false), app => RestartApp(app, true));
        AppUpdates.ShowWhatsNew = preferences.LastSeenAppVersion != AppUpdates.Version;
        AutoUpdateCommand = new(() => RunAsync(_ => { Save(); AppUpdates.SettingsChanged(); Log(state.AutoUpdate ? "Автообновление включено." : "Автообновление выключено."); return Task.CompletedTask; }), () => !IsBusy && !exiting);
    }

    public bool NeedsReleaseNotes => preferences.LastSeenAppVersion != AppUpdates.Version;
    public void MarkReleaseNotesSeen()
    {
        var previous = preferences.LastSeenAppVersion;
        try { preferences.LastSeenAppVersion = AppUpdates.Version; preferences.Save(preferencesPath); }
        catch (Exception ex) { preferences.LastSeenAppVersion = previous; Log("Не удалось сохранить просмотр истории: " + ex.Message); }
    }

    private void RestartApp(InstalledApp app, bool rollback)
    {
        if (rollback)
        {
            var previousAutoUpdate = state.AutoUpdate;
            try { state.AutoUpdate = false; Save(); }
            catch { state.AutoUpdate = previousAutoUpdate; throw; }
            Changed(nameof(AutoUpdateEnabled));
        }
        var arguments = new List<string>();
        if (runner.IsRunning) arguments.Add("--resume-discord");
        if (Telegram.IsRunning) arguments.Add("--resume-telegram");
        Program.Restart = new(app, arguments.ToArray(), rollback);
        RequestExit();
    }

    internal DiagnosticReport CreateDiagnosticReport() => new(AppUpdater.RunningVersion, Environment.OSVersion.Version,
        runner.IsRunning, IsVerified, Telegram.IsRunning, lastProbe?.Api.Skipped == true ? null : lastProbe?.Api.Success, lastProbe?.Gateway.Skipped == true ? null : lastProbe?.Gateway.Success, lastProbe?.Voice.Skipped == true ? null : lastProbe?.Voice.Success,
        lastSuccessfulCheck, Results.Count, lastError, Telegram.LastError, lastProbe?.YouTube.Site.Skipped == true ? null : lastProbe?.YouTube.Site.Success, lastProbe?.YouTube.GoogleVideo.Skipped == true ? null : lastProbe?.YouTube.GoogleVideo.Success);

    private Task ExportDiagnosticsAsync()
    {
        try
        {
            var dialog = new Microsoft.Win32.SaveFileDialog { FileName = "Svyaz-diagnostics.json", Filter = "Отчёт JSON (*.json)|*.json", DefaultExt = ".json", AddExtension = true };
            if (dialog.ShowDialog() == true)
            {
                File.WriteAllText(dialog.FileName, CreateDiagnosticReport().ToJson());
                DiagnosticFeedback = "Отчёт сохранён. Отправьте его вручную, если нужна помощь.";
            }
        }
        catch (Exception ex) { DiagnosticFeedback = "Не удалось сохранить отчёт: " + ex.Message; }
        return Task.CompletedTask;
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
        var history = AppUpdates.LoadHistoryAsync();
        var telegramInitialization = Telegram.InitializeAsync(resumeTelegram || startup && Services.Telegram);
        var appUpdate = AppUpdates.TickAsync();
        if ((startup && Services.UsesZapret) || resumeDiscord) recovery.Enable();
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
            if (recovery.Wanted && Services.UsesZapret) await ConnectAsync(ct);
            await UpdateAsync(false, ct);
        });
        await Task.WhenAll(telegramInitialization, appUpdate, history);
    }

    private Task SaveServicesAsync()
    {
        try
        {
            var next = new ServiceSelection { Discord = UseDiscord, YouTube = UseYouTube, Telegram = UseTelegram }.Validate();
            var old = preferences.Services;
            var alerts = preferences.QuietNotifications;
            try { preferences.Services = next; preferences.QuietNotifications = QuietNotifications; preferences.Save(preferencesPath); }
            catch { preferences.Services = old; preferences.QuietNotifications = alerts; throw; }
            IsVerified = false; quality.Clear(); ShowServiceResults(null);
            SelectionFeedback = "Сохранено. Общая кнопка подключает: " + SelectedServicesSummary;
            Status = "Подключим выбранные сервисы: " + SelectedServicesSummary;
            Changed(null);
        }
        catch (Exception ex) { SelectionFeedback = ex.Message; }
        return Task.CompletedTask;
    }

    private async Task ConnectSelectedAsync(bool repair = false, bool fullScan = false)
    {
        if (AnyBusy || exiting || fullScan && !Services.UsesZapret) return;
        if (fullScan) { SelectedStrategy = AutomaticStrategy; Changed(nameof(SelectedStrategy)); }
        help = null; HasError = false;
        if (Services.UsesZapret) recovery.Enable();
        var telegram = Services.Telegram ? Telegram.StartRequestedAsync() : Task.CompletedTask;
        var zapret = Services.UsesZapret ? RunAsync(ct => ConnectAsync(ct, repair, fullScan)) : Task.CompletedTask;
        await Task.WhenAll(telegram, zapret);
        if (!Services.UsesZapret) Status = Telegram.Status;
        Changed(null);
    }

    public void EnvironmentChanged()
    {
        if (exiting || disposed) return;
        var now = DateTimeOffset.UtcNow;
        environmentVersion++;
        quality.Clear();
        if (!runner.IsRunning) recovery.Invalidate(now);
        Telegram.NetworkChanged(now);
        IsVerified = false;
        if (recovery.Wanted) SetStatus(runner.IsRunning
            ? "Сеть изменилась. Текущий альт продолжает работать; новая проверка — при подключении."
            : "Сеть изменилась или Windows вышла из сна. Повторим подключение через несколько секунд.");
    }

    public async Task TickAsync()
    {
        if (ticking || exiting || disposed) return;
        ticking = true;
        try { await Task.WhenAll(TickDiscordAsync(), Telegram.TickAsync(preferences.QuietNotifications), AppUpdates.TickAsync()); }
        catch (Exception ex) { Log("Фоновая проверка: " + ex.GetType().Name); }
        finally { ticking = false; }
    }

    private async Task TickDiscordAsync()
    {
        var currentNetwork = CurrentNetwork.Read();
        if (networkId != currentNetwork) { networkId = currentNetwork; EnvironmentChanged(); }
        Changed(nameof(ConnectionTime)); Changed(nameof(NetworkSummary));
        if (IsBusy || exiting) return;
        var now = DateTimeOffset.UtcNow;
        if (IsProcessRunning && !runner.IsRunning)
        { quality.Clear(); IsVerified = false; recovery.Failed(now); SetStatus("Обход остановился. Попробуем восстановить подключение.", true); }
        if (recovery.ShouldNotify(now, preferences.QuietNotifications))
            Notify?.Invoke("Связь: соединение не восстановлено", help?.Message ?? "Откройте приложение и повторите проверку подключения.");
        if (!runner.IsRunning && recovery.TryBegin(now, NetworkInterface.GetIsNetworkAvailable()))
        {
            await RunAsync(ct => ConnectAsync(ct, true));
            return;
        }
        if (state.AutoUpdate && DateTimeOffset.UtcNow >= nextUpdateAttempt)
            await RunAsync(ct => UpdateAsync(false, ct), background: runner.IsRunning);
    }

    public async void RequestExit()
    {
        if (exiting) return;
        exiting = true;
        recovery.Stop();
        cancellation?.Cancel();
        try { await Task.WhenAll(Telegram.ExitAsync(), AppUpdates.ExitAsync()); }
        catch (Exception ex) { Log("Ошибка остановки Telegram: " + ex.Message); Telegram.Dispose(); }
        finally { telegramExited = true; CompleteExit(); }
    }

    private void CompleteExit() { if (exiting && telegramExited && cancellation == null) ExitReady?.Invoke(); }

    private async Task ResetZapretAsync()
    {
        var confirmed = false;
        await RunAsync(async ct =>
        {
            var legacy = WinDivertCleanup.FindLegacyDrivers(Path.Combine(Program.DataRoot, "Packages"));
            var extra = legacy.Count == 0 ? "" : "\n\nТакже удалить из Windows службы старого zapret?\n" +
                string.Join("\n", legacy.Select(entry => entry.Key + " — " + entry.Value));
            confirmed = System.Windows.MessageBox.Show(
                "Выгрузить и удалить драйвер WinDivert «Связи», скачанные пакеты zapret, пользовательские списки, кэш Game Filter и сохранённые альты? Текущее подключение zapret прервётся. Для загрузки пакета потребуется интернет." + extra,
                "Сбросить кэш zapret", System.Windows.MessageBoxButton.YesNo, System.Windows.MessageBoxImage.Warning,
                System.Windows.MessageBoxResult.No) == System.Windows.MessageBoxResult.Yes;
            if (!confirmed) return;
            ct.ThrowIfCancellationRequested();
            recovery.Stop();
            ResetFeedback = "Останавливаем zapret и удаляем WinDivert…";
            await runner.StopAsync();
            await WindowsRunner.EnsureNoExternalProcessAsync(ct);
            IsVerified = false;
            lastSuccessfulCheck = null;
            lastProbe = null;
            lastError = null;
            quality.Clear();
            Results.Clear();
            ShowServiceResults(null);
            help = null;
            SelectedStrategy = AutomaticStrategy;
            SetStatus(ResetFeedback);
            Log(ResetFeedback);
            await CompleteZapretResetAsync(ct, legacy);
            RefreshPackage();
            ct.ThrowIfCancellationRequested();
            ResetFeedback = "Кэш очищен. Загружаем новый пакет zapret…";
            SetStatus(ResetFeedback);
            Log(ResetFeedback);
            await UpdateAsync(true, ct);
            if (Services.UsesZapret)
            {
                recovery.Enable();
                ResetFeedback = "Пакет загружен. Идёт полный подбор альтов…";
                await ConnectAsync(ct, fullScan: true);
            }
            else SetStatus("Кэш очищен. Новый пакет готов к подключению.");
        });
        if (confirmed) ResetFeedback = Status;
    }

    private Task CompleteZapretResetAsync(CancellationToken ct, IReadOnlyDictionary<string, string>? approvedLegacy = null) =>
        ZapretCache.ResetAsync(Program.DataRoot, state, preferences, () =>
            WinDivertCleanup.RemoveAsync(Path.Combine(Program.DataRoot, "Packages"), Log, ct, approvedLegacy));

    private async Task UpdateAsync(bool force, CancellationToken ct)
    {
        if (!force && (!state.AutoUpdate || DateTimeOffset.UtcNow < nextUpdateAttempt)) return;
        nextUpdateAttempt = DateTimeOffset.UtcNow.AddHours(6);
        if (state.ZapretResetPending) await CompleteZapretResetAsync(ct);
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
            Log("Новый пакет подготовлен. Проверим и применим его при следующем подключении.");
        }
        catch (Exception ex) when (ex is HttpRequestException or IOException or System.Text.Json.JsonException)
        {
            Log("Обновление не применено: " + ex.Message);
            if (active == null && pending == null) throw;
        }
    }

    private async Task ConnectAsync(CancellationToken ct, bool repair = false, bool fullScan = false)
    {
        fullScan |= string.IsNullOrWhiteSpace(state.LastStrategy);
        if (fullScan && SelectedStrategy != AutomaticStrategy)
        {
            SelectedStrategy = AutomaticStrategy;
            Changed(nameof(SelectedStrategy));
        }
        if (!NetworkInterface.GetIsNetworkAvailable()) throw new ConnectionFailure("Нет сетевого подключения. Включите Wi-Fi или подключите кабель — повторим автоматически.");
        if (!runner.IsRunning) await WindowsRunner.EnsureNoExternalProcessAsync(ct);
        networkId = CurrentNetwork.Read();
        var startedNetwork = networkId;
        var startedVersion = environmentVersion;
        qualityEnvironmentVersion = environmentVersion;
        quality.Clear();
        var preferred = networkId != null && preferences.NetworkStrategies.TryGetValue(networkId, out var saved) ? saved : state.LastStrategy;
        var manual = repair || fullScan || SelectedStrategy == AutomaticStrategy ? null : SelectedStrategy;
        if (fullScan) Log("Полный подбор: проверяем все доступные стратегии пакета.");
        else if (repair) Log("Восстановление: запускаем быстрый подбор стратегии.");
        if (state.ActivePackage == null && state.PendingPackage == null) await UpdateAsync(true, ct);
        var active = packages.Open(state.ActivePackage);
        var pending = packages.Open(state.PendingPackage);
        var package = pending ?? active ?? throw new IOException("Сначала нужно загрузить пакет zapret с GitHub.");
        if (state.GameFilter.Mode != GameFilterMode.Disabled) await gameIpSet.EnsureAsync(ct);
        if (pending != null && active != null) PackageStore.PrepareLists(pending.Root, active.Root);
        var endpoint = Services.Discord ? GetVoiceEndpoint() : null;
        if (Services.Discord && endpoint == null && state.VoiceServer != null && state.VoiceObservedAt > DateTimeOffset.UtcNow.AddDays(-7))
            endpoint = VoiceEndpoint.Parse(state.VoiceServer);
        if (Services.Discord && endpoint == null)
        {
            SetStatus("Откройте голосовой канал Discord: ждём адрес сервера…");
            Log("Нет адреса голосового сервера. Откройте Discord и попробуйте подключиться к голосовому каналу в течение минуты. Подтверждать звук не нужно.");
            var bootstrap = !runner.IsRunning;
            try
            {
                if (bootstrap)
                {
                    var usable = false;
                    foreach (var item in Strategy.Load(package.Root, Log).OrderBy(s => s.Name == preferred ? 0 : 1))
                    {
                        ct.ThrowIfCancellationRequested();
                        try { await runner.StartAsync(package, item, ct); }
                        catch (Exception ex) when (ex is not ConnectionFailure && ex is (IOException or System.ComponentModel.Win32Exception)) { Log(ex.Message); continue; }
                        var control = await DiscordProbe.RunControlAsync(ct);
                        Log(item.Name + " (получение адреса): API " + control.Api.Detail + "; Gateway " + control.Gateway.Detail);
                        if (!control.Api.Success || !control.Gateway.Success) continue;
                        usable = true;
                        break;
                    }
                    if (!usable) throw new ConnectionFailure("Подбор завершён. Ни одна стратегия не открыла API и Gateway Discord для получения голосового адреса.", requiresManualRetry: true);
                    Log("Временная стратегия открыла API и Gateway. Теперь подключитесь к голосовому каналу; голос ещё не проверен.");
                }
                for (var attempt = 0; attempt < 20 && endpoint == null; attempt++)
                {
                    await Task.Delay(3000, ct);
                    endpoint = GetVoiceEndpoint();
                }
            }
            finally { if (bootstrap) await runner.StopAsync(); }
            if (endpoint == null) throw new ConnectionFailure("Не найден адрес голосового сервера. Войдите в канал Discord или укажите адрес в настройках.", true, requiresManualRetry: true);
        }
        if (endpoint != null) Log("Проверяем голосовой сервер: " + endpoint.Authority);
        IsVerified = false;
        Results.Clear();
        lastProbe = null;
        SetStatus(fullScan ? "Полный подбор: проверяем все альты…" : manual == null ? "Сравниваем отклик рабочих альтов. Это может занять несколько минут…" : "Проверяем выбранный альт…");
        var session = new ConnectionSession(runner, state, Save, (item, probe, outcome) =>
        {
            Report(item, probe, outcome);
            if (fullScan && probe == null && outcome.StartsWith("Запуск", StringComparison.Ordinal))
                SetStatus("Полный подбор" + outcome["Запуск".Length..] + " · " + item.Name);
        }, Log);
        var selected = await session.ConnectAsync(active, pending, new DiscordProbe(endpoint, Services, GetVoiceEndpoint).RunAsync, manual, ct, preferred,
            compareQuality: manual == null, fullScan: fullScan);
        networkId = CurrentNetwork.Read();
        if (startedNetwork != networkId || startedVersion != environmentVersion)
        {
            IsVerified = false;
            recovery.Invalidate(DateTimeOffset.UtcNow);
            SetStatus("Сеть изменилась во время проверки. Переподключитесь, чтобы проверить новый маршрут.");
            return;
        }
        if (NetworkProfiles.Remember(preferences, startedNetwork, networkId, selected.Name)) preferences.Save(preferencesPath);
        lastSuccessfulCheck = DateTimeOffset.UtcNow;
        IsVerified = true;
        recovery.Succeeded();
        SetStatus(selected.Name + " · выбранные проверки пройдены");
        Log("Два прохода выбранных проверок прошли. Оставлена стратегия " + selected.Name);
        RefreshPackage();

    }

    internal async Task RunAsync(Func<CancellationToken, Task> action, bool background = false)
    {
        if (cancellation != null || exiting) return;
        using var operation = new CancellationTokenSource();
        cancellation = operation;
        stopRequested = false;
        backgroundCheck = background;
        SetBusy(true);
        try { await action(operation.Token); }
        catch (OperationCanceledException) { quality.Clear(); IsVerified = false; Log("Операция отменена."); }
        catch (Exception ex)
        {
            IsVerified = false;
            quality.Clear();
            lastError = ex.HResult;
            Log(ex.Message);
            help = ConnectionHelp.Describe(ex, NetworkInterface.GetIsNetworkAvailable(), lastProbe);
            if (ex is ConnectionFailure { RequiresManualRetry: true }) recovery.Stop();
            else recovery.Failed(DateTimeOffset.UtcNow);
            SetStatus(help.Message, true);
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
            backgroundCheck = false;
            CompleteExit();
        }
    }

    private async Task StopAsync()
    {
        recovery.Stop();
        quality.Clear();
        await Telegram.StopRequestedAsync();
        if (cancellation != null) { stopRequested = true; cancellation.Cancel(); return; }
        await RunAsync(async _ =>
        {
            await runner.StopAsync();
            quality.Clear();
            IsVerified = false;
            ShowServiceResults(null);
            SetStatus("Отключено");
            Log("Обход остановлен.");
        });
    }

    internal void Report(Strategy item, ProbeResult? probe, string outcome)
    {
        if (probe == null && outcome.StartsWith("Запуск", StringComparison.Ordinal)) quality.Clear();
        if (probe != null)
        {
            lastProbe = probe;
            if (qualityEnvironmentVersion == environmentVersion) quality.Add(ResponseQuality.Sample(probe));
        }
        Changed(nameof(QualitySummary)); Changed(nameof(HasQuality));
        ShowServiceResults(probe, checking: probe == null && !outcome.StartsWith("Ошибка", StringComparison.Ordinal));
        var row = new StrategyResult(item.Name, probe?.Api.Detail ?? "—", probe?.Gateway.Detail ?? "—", probe?.Voice.Detail ?? "—", probe?.YouTube.Detail ?? "—", outcome, QualitySummary);
        var existing = Results.FirstOrDefault(r => r.Name == item.Name);
        if (existing == null) Results.Add(row); else Results[Results.IndexOf(existing)] = row;
        Log(item.Name + ": " + outcome + (probe == null ? "" : " | API: " + probe.Api.Detail + " | Gateway: " + probe.Gateway.Detail + " | Голос: " + probe.Voice.Detail + " | YouTube: " + probe.YouTube.Detail));
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
        RefreshCommands();
    }

    private void RefreshCommands()
    {
        RepairCommand.Refresh(); FullScanCommand.Refresh(); ConnectCommand.Refresh(); UpdateCommand.Refresh(); ResetZapretCommand.Refresh(); StopCommand.Refresh(); AutostartCommand.Refresh(); AutoUpdateCommand.Refresh(); GameFilterCommand.Refresh();
        ServicesCommand.Refresh(); HelpCommand.Refresh();
    }

    private void SetStatus(string text, bool error = false)
    {
        Status = text;
        HasError = error;
        IsProcessRunning = runner.IsRunning;
        GameFilterCommand.Refresh();
        Changed(null);

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
        recovery.Stop();
        cancellation?.Cancel();
        runner.Dispose();
        Telegram.Dispose();
        AppUpdates.Dispose();
    }
}
