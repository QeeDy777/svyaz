using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using DiscordRoute.App;
using DiscordRoute.Core;

internal static class WindowsChecks
{
    [STAThread]
    private static int Main(string[] args)
    {
        if (args.Contains("--handoff-probe")) return SelfUpdateChecks.Probe(args);
        var app = new App();
        try
        {
            app.InitializeComponent();
            var window = new MainWindow(false, initializeServices: false);
            try
            {
                Check(window.Width == 440 && window.ResizeMode == ResizeMode.CanMinimize, "fixed portrait window");
                Check(window.Icon != null, "ICO decoded by WPF");
                var tabs = Find<TabControl>(window) ?? throw new Exception("Tabs not found");
                Check(tabs.Items.Count == 4, "Discord, Telegram, settings and journal tabs");
                window.Show();
                foreach (var (index, name) in new[] { (0, "discord"), (1, "telegram"), (2, "settings"), (3, "journal") })
                {
                    tabs.SelectedIndex = index;
                    window.Dispatcher.Invoke(() => { }, System.Windows.Threading.DispatcherPriority.ApplicationIdle);
                    Check(((TabItem)tabs.Items[index]).ActualWidth > 40, name + " tab visible");
                    SaveScreenshot(window, name);
                    if (index == 0)
                    {
                        var youtube = (ContentControl)window.FindName("YouTubeCheck");
                        var page = (ScrollViewer)((TabItem)tabs.Items[0]).Content;
                        Check(youtube.IsVisible && youtube.TranslatePoint(new Point(0, youtube.ActualHeight), page).Y <= page.ActualHeight,
                            "YouTube status is visible on the fixed portrait screen without scrolling");
                    }
                }
                var model = (MainViewModel)window.DataContext;
                Check(model.RepairCommand.CanExecute(null) && model.ExportDiagnosticsCommand.CanExecute(null), "Repair and safe diagnostics actions available");
                Check(!model.ShowRepair, "Idle screen avoids a duplicate repair action");
                model.HasError = true;
                Check(model.ShowRepair, "Repair action appears when connection needs attention");
                model.HasError = false;
                Check(model.DiscordSummary.Contains("выключен") && model.Telegram.Summary.Contains("выключен"), "Separate service status starts disconnected");
                Check(model.ConnectionTime.Contains("не запущено") && model.LastCheck.Contains("пока нет"), "No invented connection time or successful check");
                Check(model.AppUpdates.CurrentNotes.Contains("0.7.1") && model.AppUpdates.EarlierNotes.Contains("0.6.1"), "Release history available offline");
                model.Report(new("fixture", "unused"), new(new(true, "sensitive-sentinel"), new(true, "sensitive-sentinel"), new(false, "sensitive-sentinel"), new(new(true, "sensitive-sentinel"), new(false, "sensitive-sentinel"))), "Failed");
                model.ShowServiceResults(null);
                var report = model.CreateDiagnosticReport();
                Check(report.VoicePassed == false && report.YouTubeGoogleVideoPassed == false && !report.ToJson().Contains("sensitive-sentinel"), "Diagnostics retain failed probes after UI reset without exporting raw details");
                var network = CurrentNetwork.Read();
                Check(network?.Length == 64, "Connected Windows CI network provides a hashed NLM identity");
                Console.WriteLine("NLM profile detected: " + (network != null));
                UpdateNoticeChecks.Run(window, tabs, Check, name => SaveScreenshot(window, name));
                Check(InstallerMaintenance.OwnTask("DiscordRoute-test", TaskXml(@"C:\App\Versions\abc\Связь.exe"), @"C:\App"), "owned autostart identified");
                Check(!InstallerMaintenance.OwnTask("DiscordRoute-test", TaskXml(@"C:\App\Versions\abc\Связь.exe").Replace("</Actions>", "<ComHandler/></Actions>"), @"C:\App"), "multi-action tasks preserved");
                Check(!InstallerMaintenance.OwnTask("Other", TaskXml(@"C:\App\Versions\abc\Связь.exe"), @"C:\App"), "other task name preserved");
                Check(!InstallerMaintenance.OwnTask("DiscordRoute-test", TaskXml(@"C:\App\Versions\..\foreign\Связь.exe"), @"C:\App"), "task path outside versions preserved");
                Check(!InstallerMaintenance.OwnTask("DiscordRoute-test", TaskXml(@"C:\App\Versions-other\Связь.exe"), @"C:\App"), "sibling path preserved");
            }
            finally { ((MainViewModel)window.DataContext).Dispose(); }
            Task.Run(() => CheckProxyAsync(args.Take(2).ToArray())).GetAwaiter().GetResult();
            SelfUpdateChecks.Run(args.Length >= 3 ? args[2] : null, Check, args.Length >= 4 ? args[3] : null);
            Console.WriteLine("Windows checks passed. Telegram messages/calls were not tested.");
            return 0;
        }
        catch (Exception ex) { Console.Error.WriteLine(ex); return 1; }
        finally { app.Shutdown(); }
    }

    private static void SaveScreenshot(Window window, string name)
    {
        // Fluent expanders animate on first display; render their settled state.
        var frame = new System.Windows.Threading.DispatcherFrame();
        var settle = new System.Windows.Threading.DispatcherTimer { Interval = TimeSpan.FromMilliseconds(500) };
        settle.Tick += (_, _) => { settle.Stop(); frame.Continue = false; };
        settle.Start();
        System.Windows.Threading.Dispatcher.PushFrame(frame);
        window.Dispatcher.Invoke(() => { }, System.Windows.Threading.DispatcherPriority.ApplicationIdle);
        if (name == "settings")
        {
            var notes = (TextBlock)window.FindName("CurrentNotesBlock");
            var history = (Expander)window.FindName("ReleaseHistory");
            Check(notes.TranslatePoint(new Point(0, 0), history).Y >= 35, "Release notes begin below the expander header after animation");
        }
        var content = (FrameworkElement)window.Content;
        content.UpdateLayout();
        var bitmap = new RenderTargetBitmap(440, 680, 96, 96, PixelFormats.Pbgra32);
        var visual = new DrawingVisual();
        using (var drawing = visual.RenderOpen())
        {
            drawing.DrawRectangle(window.Background ?? Brushes.White, null, new Rect(0, 0, 440, 680));
            drawing.DrawRectangle(new VisualBrush(content), null, new Rect(content.TranslatePoint(new Point(0, 0), window), new Size(content.ActualWidth, content.ActualHeight)));
        }
        bitmap.Render(visual);
        var encoder = new PngBitmapEncoder();
        encoder.Frames.Add(BitmapFrame.Create(bitmap));
        Directory.CreateDirectory("dist");
        using var screenshot = File.Create($"dist/{name}-window.png");
        encoder.Save(screenshot);
    }

    private static string TaskXml(string path) => "<Task xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\"><Actions><Exec><Command>" + path + "</Command></Exec></Actions></Task>";

    private static T? Find<T>(DependencyObject parent) where T : DependencyObject
    {
        if (parent is T match) return match;
        foreach (var child in LogicalTreeHelper.GetChildren(parent).OfType<DependencyObject>())
            if (Find<T>(child) is { } found) return found;
        return null;
    }

    private static async Task CheckProxyAsync(string[] args)
    {
        var root = Path.Combine(Path.GetTempPath(), "svyaz-windows-" + Guid.NewGuid().ToString("N"));
        try
        {
            var store = new TelegramPackageStore(root, Console.WriteLine);
            using var timeout = new CancellationTokenSource(TimeSpan.FromMinutes(4));
            if (args.Length != 0 && args.Length != 2) throw new ArgumentException("Expected release JSON and official EXE paths");
            var package = args.Length == 2
                ? store.Install(TelegramPackageStore.ParseRelease(File.ReadAllText(args[0])), File.ReadAllBytes(args[1]))
                : await store.StageLatestAsync(null, timeout.Token) ?? throw new Exception("Missing package");
            var listener = new TcpListener(IPAddress.Loopback, 0);
            listener.Server.ExclusiveAddressUse = true;
            listener.Start();
            var port = ((IPEndPoint)listener.LocalEndpoint).Port;
            var settings = new TelegramSettings { Port = port }.WithSecret();
            settings.WriteConfiguration(package.Root);
            using var runner = new TelegramRunner();
            try
            {
                try { await runner.StartAsync(package, settings, timeout.Token); throw new Exception("Busy port accepted"); }
                catch (IOException ex) when (ex.InnerException is SocketException) { }
                Check(!runner.IsRunning && listener.Server.IsBound, "foreign listener left intact");
            }
            finally { listener.Stop(); }
            await runner.StartAsync(package, settings, timeout.Token);
            Check(runner.IsRunning, "official Telegram EXE running and local port responds");
            await runner.StopAsync();
            Check(!runner.IsRunning, "Telegram process stopped");
            Check(!Directory.EnumerateDirectories(TelegramRunner.TempRoot).Any(), "PyInstaller extraction cleaned after stop");
            listener = new TcpListener(IPAddress.Loopback, port);
            listener.Server.ExclusiveAddressUse = true;
            try { listener.Start(); Console.WriteLine("PASS: child process released proxy port"); }
            finally { listener.Stop(); }
        }
        finally { if (Directory.Exists(root)) Directory.Delete(root, true); }
    }

    private static void Check(bool success, string name)
    {
        if (!success) throw new Exception(name);
        Console.WriteLine("PASS: " + name);
    }
}
