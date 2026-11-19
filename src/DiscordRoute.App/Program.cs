using System.IO;
using System.Windows;
using System.Diagnostics;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal static class Program
{
    private const string ParentRefreshesIcons = "--parent-refreshes-shortcut-icons";
    // Keep the original location so a display-name change preserves existing settings and packages.
    internal static string InstallRoot => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "DiscordRoute");
    internal static string DataRoot => Path.Combine(InstallRoot, "Data");
    internal static UpdateRestart? Restart { get; set; }

    [STAThread]
    private static void Main(string[] args)
    {
        var maintenance = args.Length == 1 && args[0] is "--install" or "--uninstall";
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
                    if (maintenance) { Environment.ExitCode = 1; return; }
                    if (readyToken == null) MessageBox.Show("«Связь» уже запущена. Откройте приложение через значок в трее.", "Связь");
                    return;
                }
                try
                {
                    Directory.CreateDirectory(DataRoot);
                    if (maintenance)
                    {
                        if (args[0] == "--install") InstallerMaintenance.Install();
                        else InstallerMaintenance.Uninstall();
                        return;
                    }
                    if (readyToken == null && !args.Contains("--skip-self-update"))
                    {
                        try
                        {
                            var selected = updater.SelectForLaunch(AppState.Load(Path.Combine(DataRoot, "settings.json")).AutoUpdate);
                            if (selected != null) { updater.Executable(selected); Restart = new(selected, resume, AppUpdatePackage.ParseVersion(selected.Version) < AppUpdater.RunningVersion); }
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
                            acknowledge = (_, _) =>
                            {
                                window.Loaded -= acknowledge;
                                UpdateHandoff.Signal(readyToken);
                                if (!args.Contains(ParentRefreshesIcons)) _ = RefreshAfterLegacyHandoffAsync(updater, readyToken);
                            };
                            window.Loaded += acknowledge;
                        }
                        application.Run(window);
                    }
                }
                finally { mutex.ReleaseMutex(); }
            }
            if (Restart is { } restart)
            {
                string executable;
                try
                {
                    executable = updater.Executable(restart.App);
                    if (!UpdateHandoff.Start(executable, [.. restart.Arguments, ParentRefreshesIcons],
                        () => updater.Promote(restart.App, restart.Rollback), TimeSpan.FromSeconds(30)))
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
                    return;
                }
                ShortcutIcons.Refresh(InstallRoot, executable);
            }
        }
        catch (Exception ex)
        {
            var logPath = StartupFailure.Save(ex, DataRoot) ?? StartupFailure.Save(ex,
                Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Svyaz"));
            if (maintenance) { Environment.ExitCode = 1; return; }
            if (readyToken != null) return;
            var cause = ex.GetBaseException();
            var details = logPath == null ? "Журнал ошибки сохранить не удалось." : "Подробности: " + logPath;
            MessageBox.Show("Не удалось запустить «Связь»:\n" + cause.GetType().Name + ": " + cause.Message +
                "\n\n" + details, "Связь", MessageBoxButton.OK, MessageBoxImage.Error);
        }
    }

    private static async Task RefreshAfterLegacyHandoffAsync(AppUpdater updater, string token)
    {
        // A pre-0.13.1 parent closes the ready event after it commits current.json.
        var executable = Environment.ProcessPath!;
        var versionDirectory = Path.GetDirectoryName(executable)!;
        if (!string.Equals(Path.GetDirectoryName(versionDirectory), Path.Combine(InstallRoot, "Versions"), StringComparison.OrdinalIgnoreCase)) return;
        try
        {
            await UpdateHandoff.WaitForParentAsync(token);
            if (updater.Read("current")?.Id == Path.GetFileName(versionDirectory))
                ShortcutIcons.Refresh(InstallRoot, executable);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or System.Text.Json.JsonException or InvalidDataException or FormatException)
        {
            System.Diagnostics.Trace.WriteLine(ex);
        }
    }
}
