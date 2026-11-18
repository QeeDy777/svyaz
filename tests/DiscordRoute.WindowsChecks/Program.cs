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
                tabs.SelectedIndex = 0;
                window.UpdateLayout();
                var fullScan = (Button)window.FindName("FullScanButton");
                Check(fullScan.Command == model.FullScanCommand && model.FullScanCommand.CanExecute(null), "Full scan action is bound and available for zapret services");
                fullScan.BringIntoView();
                SaveScreenshot(window, "full-selection");
                Check(model.RepairCommand.CanExecute(null) && model.ExportDiagnosticsCommand.CanExecute(null), "Repair and safe diagnostics actions available");
                Check(!model.ShowRepair, "Idle screen avoids a duplicate repair action");
                model.HasError = true;
                Check(model.ShowRepair, "Repair action appears when connection needs attention");
                model.HasError = false;
                Check(model.DiscordSummary.Contains("выключен") && model.Telegram.Summary.Contains("выключен"), "Separate service status starts disconnected");
                Check(model.ConnectionTime.Contains("не запущено") && model.LastCheck.Contains("пока нет"), "No invented connection time or successful check");
                Check(model.AppUpdates.CurrentNotes.Contains("0.11.4") && model.AppUpdates.EarlierNotes.Contains("0.6.1"), "Release history available offline");
                model.Report(new("fixture", "unused"), new(new(true, "sensitive-sentinel"), new(true, "sensitive-sentinel"), new(false, "sensitive-sentinel"), new(new(true, "sensitive-sentinel"), new(false, "sensitive-sentinel"))), "Failed");
                model.ShowServiceResults(null);
                var report = model.CreateDiagnosticReport();
                Check(report.VoicePassed == false && report.YouTubeGoogleVideoPassed == false && !report.ToJson().Contains("sensitive-sentinel"), "Diagnostics retain failed probes after UI reset without exporting raw details");
                model.Report(new("quality-fixture", "unused"), null, "Запуск");
                foreach (var ms in new long[] { 90, 100, 110 })
                {
                    var timed = new CheckResult(true, "OK", ElapsedMs: ms);
                    model.Report(new("quality-fixture", "unused"), new(timed, timed, timed, new(timed, timed)), "Проверка");
                }
                Check(model.HasQuality && model.QualitySummary.Contains("100 мс") && model.QualitySummary.Contains("20 мс"), "Typed response timings reach the quality panel");
                tabs.SelectedIndex = 0;
                ((ScrollViewer)((TabItem)tabs.Items[0]).Content).ScrollToBottom();
                window.UpdateLayout();
                Check(((FrameworkElement)window.FindName("QualityPanel")).IsVisible, "Quality panel is visible after measured probes");
                SaveScreenshot(window, "connection-quality");
                model.EnvironmentChanged();
                Check(!model.HasQuality, "Network change clears obsolete response quality");
                var network = CurrentNetwork.Read();
                Check(network?.Length == 64, "Connected Windows CI network provides a hashed NLM identity");
                Console.WriteLine("NLM profile detected: " + (network != null));
                QuietCheckTests.Run(window, tabs, model, Check, name => SaveScreenshot(window, name));
                CheckServiceSelection(window, tabs, model);
                UpdateNoticeChecks.Run(window, tabs, Check, name => SaveScreenshot(window, name));
                Check(InstallerMaintenance.OwnTask("DiscordRoute-test", TaskXml(@"C:\App\Versions\abc\Связь.exe"), @"C:\App"), "owned autostart identified");
                Check(!InstallerMaintenance.OwnTask("DiscordRoute-test", TaskXml(@"C:\App\Versions\abc\Связь.exe").Replace("</Actions>", "<ComHandler/></Actions>"), @"C:\App"), "multi-action tasks preserved");
                Check(!InstallerMaintenance.OwnTask("Other", TaskXml(@"C:\App\Versions\abc\Связь.exe"), @"C:\App"), "other task name preserved");
                Check(!InstallerMaintenance.OwnTask("DiscordRoute-test", TaskXml(@"C:\App\Versions\..\foreign\Связь.exe"), @"C:\App"), "task path outside versions preserved");
                Check(!InstallerMaintenance.OwnTask("DiscordRoute-test", TaskXml(@"C:\App\Versions-other\Связь.exe"), @"C:\App"), "sibling path preserved");
            }
            finally { ((MainViewModel)window.DataContext).Dispose(); }
            Task.Run(() => ProcessHandoffChecks.RunAsync(Check)).GetAwaiter().GetResult();
            Task.Run(() => CheckProxyAsync()).GetAwaiter().GetResult();
            SelfUpdateChecks.Run(args.Length >= 1 ? args[0] : null, Check, args.Length >= 2 ? args[1] : null);
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

    private static void CheckServiceSelection(MainWindow window, TabControl tabs, MainViewModel model)
    {
        var preferences = Path.Combine(DiscordRoute.App.Program.DataRoot, "preferences.json");
        var original = File.Exists(preferences) ? File.ReadAllBytes(preferences) : null;
        var oldPort = model.Telegram.Port;
        try
        {
            model.UseDiscord = false; model.UseYouTube = false; model.UseTelegram = true;
            model.ServicesCommand.Execute(null);
            Check(!model.Services.UsesZapret && model.Services.Telegram, "Telegram-only selection saved without enabling zapret");
            Check(!model.FullScanCommand.CanExecute(null), "Full scan is unavailable for Telegram-only selection");
            tabs.SelectedIndex = 0; window.UpdateLayout();
            Check(!((ContentControl)window.FindName("YouTubeCheck")).IsVisible, "Unselected YouTube check is hidden");
            Check(!model.Heading.Contains("отвечают"), "Idle Telegram-only selection never claims success");
            model.UseTelegram = false; model.ServicesCommand.Execute(null);
            Check(model.Services.Telegram && model.SelectionFeedback.Contains("хотя бы"), "Empty selection preserves previous settings");
            var alerts = 0;
            model.Notify += (_, _) => alerts++;
            model.Telegram.Port = "invalid";
            model.ConnectCommand.Execute(null);
            Check(model.Telegram.LastError != null && model.Telegram.Recovery.Wanted && !model.IsProcessRunning,
                "Common connect reaches only selected Telegram and keeps failure actionable");
            Check(alerts == 0 && model.Telegram.HasFailure, "Brief startup error is visible without a balloon");
            Check(model.ConnectionStatus.Contains("порт") && model.Heading.Contains("внимание") && model.HelpActionLabel == "Открыть настройки",
                "Selected Telegram failure is actionable on the common screen");
            model.StopCommand.Execute(null);
            Check(!model.Telegram.Recovery.Wanted && !model.ShowDisconnect, "Common disconnect cancels Telegram recovery even after failed start");
            model.EnvironmentChanged();
            Check(!model.ShowDisconnect, "Network change does not start manually disabled services");
            model.UseYouTube = true; model.UseTelegram = false;
            model.ServicesCommand.Execute(null);
            Check(model.Services.YouTube && !model.Services.Discord && !model.Services.Telegram, "YouTube-only mode is independently selectable");
            Check(model.FullScanCommand.CanExecute(null), "Full scan is available for YouTube-only selection");
            model.UseDiscord = true; model.ServicesCommand.Execute(null);
            model.Telegram.Port = oldPort;
            tabs.SelectedIndex = 2;
            ((ScrollViewer)((TabItem)tabs.Items[2]).Content).ScrollToTop();
            ((Expander)window.FindName("ReleaseHistory")).IsExpanded = false;
            SaveScreenshot(window, "service-selection");
        }
        finally
        {
            model.Telegram.Port = oldPort;
            if (original != null) File.WriteAllBytes(preferences, original);
            else if (File.Exists(preferences)) File.Delete(preferences);
        }
    }

    private static async Task CheckProxyAsync()
    {
        using var timeout = new CancellationTokenSource(TimeSpan.FromMinutes(2));
        var listener = new TcpListener(IPAddress.Loopback, 0);
        listener.Server.ExclusiveAddressUse = true;
        listener.Start();
        var port = ((IPEndPoint)listener.LocalEndpoint).Port;
        var settings = new TelegramSettings { Port = port }.WithSecret();
        var configuration = settings.Configuration("{\"pool_size\":0,\"cfproxy\":false}");
        var existing = System.Diagnostics.Process.GetProcessesByName("TgWsProxy_windows").Select(p => p.Id).ToHashSet();
        using var runner = new TelegramRunner();
        try
        {
            try { await runner.StartAsync(configuration, settings, timeout.Token); throw new Exception("Busy port accepted"); }
            catch (IOException ex) when (ex.InnerException is SocketException) { }
            Check(!runner.IsRunning && listener.Server.IsBound, "foreign listener left intact");
        }
        finally { listener.Stop(); }
        for (var run = 0; run < 3; run++)
        {
            await runner.StartAsync(configuration, settings, timeout.Token);
            Check(runner.IsRunning, "embedded Telegram running, pass " + run);
            using (var client = new TcpClient())
            {
                await client.ConnectAsync(IPAddress.Loopback, port, timeout.Token);
                await client.GetStream().WriteAsync(new byte[64], timeout.Token);
            }
            Check(System.Diagnostics.Process.GetCurrentProcess().Modules.Cast<System.Diagnostics.ProcessModule>().Any(m => m.ModuleName == "python314.dll"),
                "Python DLL is loaded inside the current process");
            Check(System.Diagnostics.Process.GetProcessesByName("TgWsProxy_windows").All(p => existing.Contains(p.Id)), "no separate TG WS Proxy process launched");
            if (run == 0)
            {
                using var cancelProbe = new CancellationTokenSource(TimeSpan.FromMilliseconds(50));
                try { await runner.ProbeAsync(cancelProbe.Token); throw new Exception("Probe cancellation ignored"); }
                catch (OperationCanceledException) { Check(runner.IsRunning, "Server probe cancellation preserves local proxy"); }
            }
            await runner.StopAsync();
            Check(!runner.IsRunning, "embedded Telegram stopped");
            listener = new TcpListener(IPAddress.Loopback, port);
            listener.Server.ExclusiveAddressUse = true;
            try { listener.Start(); Check(true, "embedded engine released proxy port"); }
            finally { listener.Stop(); }
        }
        using var cancelled = new CancellationTokenSource();
        cancelled.Cancel();
        try { await runner.StartAsync(configuration, settings, cancelled.Token); throw new Exception("Cancellation ignored"); }
        catch (OperationCanceledException) { Check(!runner.IsRunning, "cancelled start does not leave a proxy running"); }
        var runtime = TelegramRuntime.Prepare(Path.Combine(Program.DataRoot, "Telegram", "Embedded"));
        Check(!Directory.EnumerateFiles(runtime, "*.exe", SearchOption.AllDirectories).Any(), "embedded runtime contains no executable launchers");
        var adapter = Path.Combine(runtime, "svyaz_proxy.py");
        var originalAdapter = File.ReadAllBytes(adapter);
        try
        {
            File.WriteAllText(adapter, "tampered");
            try { TelegramRuntime.Prepare(Path.GetDirectoryName(runtime)!); throw new Exception("Tampered engine accepted"); }
            catch (IOException) { Check(true, "modified extracted engine is rejected before native loading"); }
        }
        finally { File.WriteAllBytes(adapter, originalAdapter); }
    }

    private static void Check(bool success, string name)
    {
        if (!success) throw new Exception(name);
        Console.WriteLine("PASS: " + name);
    }
}
