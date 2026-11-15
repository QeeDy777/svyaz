using System.IO;
using System.Windows;
using System.Diagnostics;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal static class Program
{
    // Keep the original location so a display-name change preserves existing settings and packages.
    internal static string InstallRoot => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "DiscordRoute");
    internal static string DataRoot => Path.Combine(InstallRoot, "Data");
    internal static UpdateRestart? Restart { get; set; }

    [STAThread]
    private static void Main(string[] args)
    {
        var readyIndex = Array.IndexOf(args, "--update-ready");
        var readyToken = readyIndex >= 0 && readyIndex + 1 < args.Length ? args[readyIndex + 1] : null;
        var resume = args.Where(a => a is "--startup" or "--resume-discord" or "--resume-telegram").ToArray();
        var updater = new AppUpdater();
        try
        {
            using (var mutex = new Mutex(true, @"Global\DiscordRoute", out var created))
            {
                if (!created)
                {
                    if (readyToken == null) MessageBox.Show("«Связь» уже запущена. Откройте приложение через значок в трее.", "Связь");
                    return;
                }
                try
                {
                    Directory.CreateDirectory(DataRoot);
                    if (readyToken == null && !args.Contains("--skip-self-update"))
                    {
                        try
                        {
                            var selected = updater.SelectForLaunch(AppState.Load(Path.Combine(DataRoot, "settings.json")).AutoUpdate);
                            if (selected != null) { updater.Executable(selected); Restart = new(selected, resume); }
                        }
                        catch (Exception ex) { StartupFailure.Save(ex, DataRoot); }
                    }
                    if (Restart == null)
                    {
                        var application = new App();
                        application.InitializeComponent();
                        var window = new MainWindow(args.Contains("--startup"), true, args.Contains("--resume-discord"), args.Contains("--resume-telegram"));
                        if (readyToken != null)
                        {
                            RoutedEventHandler? acknowledge = null;
                            acknowledge = (_, _) => { window.Loaded -= acknowledge; UpdateHandoff.Signal(readyToken); };
                            window.Loaded += acknowledge;
                        }
                        application.Run(window);
                    }
                }
                finally { mutex.ReleaseMutex(); }
            }
            if (Restart is { } restart)
            {
                try
                {
                    if (UpdateHandoff.Start(updater.Executable(restart.App), restart.Arguments,
                        () => updater.Promote(restart.App), TimeSpan.FromSeconds(30))) return;
                    throw new IOException("Новая версия не подтвердила запуск за 30 секунд.");
                }
                catch (Exception ex)
                {
                    StartupFailure.Save(ex, DataRoot);
                    try { updater.Reject(restart.App); } catch (Exception failure) { StartupFailure.Save(failure, DataRoot); }
                    var fallback = new ProcessStartInfo(Environment.ProcessPath!) { UseShellExecute = false };
                    fallback.ArgumentList.Add("--skip-self-update");
                    foreach (var argument in restart.Arguments) fallback.ArgumentList.Add(argument);
                    Process.Start(fallback);
                    MessageBox.Show("Обновление не запустилось. Возвращаем предыдущую версию «Связи». Подробности — в startup-error.log.", "Связь");
                }
            }
        }
        catch (Exception ex)
        {
            var logPath = StartupFailure.Save(ex, DataRoot) ?? StartupFailure.Save(ex,
                Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Svyaz"));
            if (readyToken != null) return;
            var cause = ex.GetBaseException();
            var details = logPath == null ? "Журнал ошибки сохранить не удалось." : "Подробности: " + logPath;
            MessageBox.Show("Не удалось запустить «Связь»:\n" + cause.GetType().Name + ": " + cause.Message +
                "\n\n" + details, "Связь", MessageBoxButton.OK, MessageBoxImage.Error);
        }
    }
}
