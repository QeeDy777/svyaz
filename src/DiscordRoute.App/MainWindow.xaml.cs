using System.ComponentModel;
using System.Net.NetworkInformation;
using Microsoft.Win32;
using System.Windows;
using System.Windows.Threading;
using Forms = System.Windows.Forms;

namespace DiscordRoute.App;

public partial class MainWindow : Window
{
    private readonly MainViewModel model;
    private readonly Forms.NotifyIcon tray;
    private readonly System.Drawing.Icon trayIcon;
    private readonly Forms.ContextMenuStrip trayMenu = new();
    private readonly DispatcherTimer timer = new() { Interval = TimeSpan.FromSeconds(15) };
    private bool exiting;
    private bool initialized;
    private readonly bool servicesEnabled;

    public MainWindow(bool startup) : this(startup, true) { }

    internal MainWindow(bool startup, bool initializeServices, bool resumeDiscord = false, bool resumeTelegram = false)
    {
        initialized = !initializeServices;
        servicesEnabled = initializeServices;
        InitializeComponent();
        Height = Math.Min(Height, SystemParameters.WorkArea.Height);
        model = new(action => { if (!Dispatcher.HasShutdownStarted) Dispatcher.BeginInvoke(action); });
        DataContext = model;
        using (var stream = Application.GetResourceStream(new Uri($"pack://application:,,,/{typeof(App).Assembly.GetName().Name};component/Assets/app.ico"))!.Stream)
        using (var icon = new System.Drawing.Icon(stream, 32, 32))
            trayIcon = (System.Drawing.Icon)icon.Clone();
        tray = new() { Icon = trayIcon, Text = "Связь", Visible = true, ContextMenuStrip = trayMenu };
        trayMenu.Items.Add("Открыть", null, (_, _) => ShowWindow());
        trayMenu.Items.Add("Подключить выбранные сервисы", null, (_, _) => model.ConnectCommand.Execute(null));
        var repairItem = trayMenu.Items.Add("Исправить подключение", null, (_, _) => model.RepairCommand.Execute(null));
        var updateItem = trayMenu.Items.Add("Проверить обновление «Связи»", null, (_, _) => model.AppUpdates.CheckCommand.Execute(null));
        var strategyItem = trayMenu.Items.Add("Стратегия: —");
        strategyItem.Enabled = false;
        trayMenu.Opening += (_, _) =>
        {
            repairItem.Enabled = model.RepairCommand.CanExecute(null);
            updateItem.Enabled = model.AppUpdates.CheckCommand.CanExecute(null);
            strategyItem.Text = "Стратегия: " + model.CurrentStrategy;
        };
        trayMenu.Items.Add("Отключить", null, (_, _) => model.StopCommand.Execute(null));
        trayMenu.Items.Add("Запустить / остановить Telegram", null, (_, _) => model.Telegram.PrimaryCommand.Execute(null));
        trayMenu.Items.Add(new Forms.ToolStripSeparator());
        trayMenu.Items.Add("Выход (остановить всё)", null, (_, _) => model.RequestExit());
        tray.BalloonTipClicked += (_, _) => ShowWindow();
        model.AppUpdates.UpdateReady += title =>
        {
            if (!IsVisible) tray.ShowBalloonTip(7000, title, "Обновление загружено. Нажмите, чтобы открыть «Связь» и перезапустить её.", Forms.ToolTipIcon.Info);
        };
        tray.DoubleClick += (_, _) => ShowWindow();
        model.Notify += (title, text) => { if (!IsVisible) tray.ShowBalloonTip(5000, title, text, Forms.ToolTipIcon.Info); };
        model.SettingsRequested += () => { ShowWindow(); MainTabs.SelectedIndex = 2; VoiceInput.BringIntoView(); VoiceInput.Focus(); };
        model.Telegram.SettingsRequested += () => { ShowWindow(); MainTabs.SelectedIndex = 1; TelegramOptions.IsExpanded = true; TelegramOptions.BringIntoView(); };
        model.ExitReady += () => { exiting = true; Close(); };
        void UpdateTray() => tray.Text = "Связь · Discord: " + (!model.Services.Discord ? "выкл" : model.IsVerified ? "вкл" : model.IsProcessRunning ? "?" : "выкл") +
            " · Telegram: " + (model.Telegram.IsRunning ? "вкл" : "выкл");
        model.PropertyChanged += (_, _) => UpdateTray();
        model.Telegram.PropertyChanged += (_, _) => UpdateTray();
        Loaded += async (_, _) =>
        {
            if (initialized) return;
            initialized = true;
            if (startup) HideToTray();
            else ShowReleaseNotes();
            model.AppUpdates.AnnouncePending();
            await model.InitializeAsync(startup, resumeDiscord, resumeTelegram);
            if (!exiting) timer.Start();
        };
        timer.Tick += async (_, _) => await model.TickAsync();
        StateChanged += (_, _) => { if (WindowState == WindowState.Minimized) HideToTray(); };
        Application.Current.SessionEnding += OnSessionEnding;
        if (servicesEnabled)
        {
            SystemEvents.PowerModeChanged += OnPowerModeChanged;
            NetworkChange.NetworkAddressChanged += OnNetworkChanged;
        }
    }

    private void OnPowerModeChanged(object sender, PowerModeChangedEventArgs e)
    {
        if (e.Mode == PowerModes.Resume) OnNetworkChanged(sender, EventArgs.Empty);
    }
    private void OnNetworkChanged(object? sender, EventArgs e)
    {
        if (!Dispatcher.HasShutdownStarted) Dispatcher.BeginInvoke(() => { if (!exiting) model.EnvironmentChanged(); });
    }

    private void HideToTray() { Hide(); ShowInTaskbar = false; }
    private void ShowWindow() { ShowInTaskbar = true; Show(); WindowState = WindowState.Normal; Activate(); ShowReleaseNotes(); }
    private void ShowReleaseNotes()
    {
        if (!servicesEnabled || !model.NeedsReleaseNotes || !IsVisible) return;
        MainTabs.SelectedIndex = 2;
        model.MarkReleaseNotesSeen();
    }
    private void OnSessionEnding(object sender, SessionEndingCancelEventArgs e) { exiting = true; model.Dispose(); }
    protected override void OnClosing(CancelEventArgs e)
    {
        if (!exiting) { e.Cancel = true; HideToTray(); }
        base.OnClosing(e);
    }
    protected override void OnClosed(EventArgs e)
    {
        timer.Stop();
        if (servicesEnabled)
        {
            SystemEvents.PowerModeChanged -= OnPowerModeChanged;
            NetworkChange.NetworkAddressChanged -= OnNetworkChanged;
        }
        Application.Current.SessionEnding -= OnSessionEnding;
        model.Dispose();
        tray.Dispose();
        trayIcon.Dispose();
        trayMenu.Dispose();
        base.OnClosed(e);
    }
}
