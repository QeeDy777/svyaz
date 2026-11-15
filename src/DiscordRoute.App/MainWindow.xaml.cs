using System.ComponentModel;
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
    private readonly DispatcherTimer timer = new() { Interval = TimeSpan.FromMinutes(1) };
    private bool exiting;
    private bool initialized;

    public MainWindow(bool startup)
    {
        InitializeComponent();
        Height = Math.Min(Height, SystemParameters.WorkArea.Height);
        model = new(action => { if (!Dispatcher.HasShutdownStarted) Dispatcher.BeginInvoke(action); });
        DataContext = model;
        using (var stream = Application.GetResourceStream(new Uri("pack://application:,,,/Assets/app.ico"))!.Stream)
        using (var icon = new System.Drawing.Icon(stream, 32, 32))
            trayIcon = (System.Drawing.Icon)icon.Clone();
        tray = new() { Icon = trayIcon, Text = "Связь", Visible = true, ContextMenuStrip = trayMenu };
        trayMenu.Items.Add("Открыть", null, (_, _) => ShowWindow());
        trayMenu.Items.Add("Подобрать и подключить", null, (_, _) => model.ConnectCommand.Execute(null));
        trayMenu.Items.Add("Отключить", null, (_, _) => model.StopCommand.Execute(null));
        trayMenu.Items.Add(new Forms.ToolStripSeparator());
        trayMenu.Items.Add("Выход (отключить обход)", null, (_, _) => model.RequestExit());
        tray.DoubleClick += (_, _) => ShowWindow();
        model.Notify += (title, text) => { if (!IsVisible) tray.ShowBalloonTip(5000, title, text, Forms.ToolTipIcon.Info); };
        model.ExitReady += () => { exiting = true; Close(); };
        model.PropertyChanged += (_, _) => tray.Text = "Связь · " + (model.IsVerified ? "Подключено" : model.IsProcessRunning ? "Не проверено" : "Отключено");
        Loaded += async (_, _) =>
        {
            if (initialized) return;
            initialized = true;
            if (startup) HideToTray();
            await model.InitializeAsync(startup);
            if (!exiting) timer.Start();
        };
        timer.Tick += async (_, _) => await model.TickAsync();
        StateChanged += (_, _) => { if (WindowState == WindowState.Minimized) HideToTray(); };
        Application.Current.SessionEnding += OnSessionEnding;
    }

    private void HideToTray() { Hide(); ShowInTaskbar = false; }
    private void ShowWindow() { ShowInTaskbar = true; Show(); WindowState = WindowState.Normal; Activate(); }
    private void OnSessionEnding(object sender, SessionEndingCancelEventArgs e) { exiting = true; model.Dispose(); }
    protected override void OnClosing(CancelEventArgs e)
    {
        if (!exiting) { e.Cancel = true; HideToTray(); }
        base.OnClosing(e);
    }
    protected override void OnClosed(EventArgs e)
    {
        timer.Stop();
        Application.Current.SessionEnding -= OnSessionEnding;
        model.Dispose();
        tray.Dispose();
        trayIcon.Dispose();
        trayMenu.Dispose();
        base.OnClosed(e);
    }
}
