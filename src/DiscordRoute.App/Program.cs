using System.IO;
using System.Windows;

namespace DiscordRoute.App;

internal static class Program
{
    // Keep the original location so a display-name change preserves existing settings and packages.
    internal static string InstallRoot => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "DiscordRoute");
    internal static string DataRoot => Path.Combine(InstallRoot, "Data");

    [STAThread]
    private static void Main(string[] args)
    {
        using var mutex = new Mutex(true, @"Global\DiscordRoute", out var created);
        if (!created)
        {
            MessageBox.Show("«Связь» уже запущена. Откройте приложение через значок в трее.", "Связь");
            return;
        }
        try
        {
            Directory.CreateDirectory(DataRoot);
            var application = new App();
            application.InitializeComponent();
            application.Run(new MainWindow(args.Contains("--startup")));
        }
        catch (Exception ex)
        {
            var logPath = StartupFailure.Save(ex, DataRoot) ?? StartupFailure.Save(ex,
                Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Svyaz"));
            var cause = ex.GetBaseException();
            var details = logPath == null ? "Журнал ошибки сохранить не удалось." : "Подробности: " + logPath;
            MessageBox.Show("Не удалось запустить «Связь»:\n" + cause.GetType().Name + ": " + cause.Message +
                "\n\n" + details, "Связь", MessageBoxButton.OK, MessageBoxImage.Error);
        }
    }
}
