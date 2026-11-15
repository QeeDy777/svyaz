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
                tabs.SelectedIndex = 1;
                window.Show();
                window.Dispatcher.Invoke(() => { }, System.Windows.Threading.DispatcherPriority.ApplicationIdle);
                var content = (FrameworkElement)window.Content;
                content.UpdateLayout();
                var bitmap = new RenderTargetBitmap(440, 680, 96, 96, PixelFormats.Pbgra32);
                var visual = new DrawingVisual();
                using (var drawing = visual.RenderOpen())
                {
                    drawing.DrawRectangle(window.Background ?? Brushes.White, null, new Rect(0, 0, 440, 680));
                    drawing.DrawRectangle(new VisualBrush(content), null, new Rect(0, 0, content.ActualWidth, content.ActualHeight));
                }
                bitmap.Render(visual);
                var encoder = new PngBitmapEncoder();
                encoder.Frames.Add(BitmapFrame.Create(bitmap));
                Directory.CreateDirectory("dist");
                using var screenshot = File.Create("dist/telegram-window.png");
                encoder.Save(screenshot);
                Console.WriteLine("PASS: Telegram tab rendered");
            }
            finally { ((MainViewModel)window.DataContext).Dispose(); }
            Task.Run(() => CheckProxyAsync(args)).GetAwaiter().GetResult();
            Console.WriteLine("Windows checks passed. Telegram messages/calls were not tested.");
            return 0;
        }
        catch (Exception ex) { Console.Error.WriteLine(ex); return 1; }
        finally { app.Shutdown(); }
    }

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
