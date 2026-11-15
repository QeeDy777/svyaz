using System.Diagnostics;
using System.Security.Principal;
using System.Text;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal static class Autostart
{
    private static string Sid => WindowsIdentity.GetCurrent().User!.Value;
    private static string Name => "DiscordRoute-" + Sid;

    public static async Task<bool> IsEnabledAsync() => await RunAsync("/Query", "/TN", Name) == 0;

    public static async Task SetAsync(bool enabled)
    {
        if (!enabled)
        {
            if (await IsEnabledAsync() && await RunAsync("/Delete", "/TN", Name, "/F") != 0)
                throw new IOException("Не удалось удалить задачу автозапуска.");
            return;
        }
        var installed = InstallExecutable();
        var xmlPath = Path.Combine(Program.DataRoot, "autostart.xml");
        try
        {
            File.WriteAllText(xmlPath, LogonTask.Xml(installed, Sid), Encoding.Unicode);
            if (await RunAsync("/Create", "/TN", Name, "/XML", xmlPath, "/F") != 0)
                throw new IOException("Планировщик Windows не создал задачу автозапуска.");
        }
        finally { File.Delete(xmlPath); }
    }

    public static string InstallExecutable() => RuntimeInstallation.Install(
        Environment.ProcessPath ?? throw new IOException("Не найден путь приложения."), Program.InstallRoot);

    private static async Task<int> RunAsync(params string[] arguments)
    {
        var start = new ProcessStartInfo(Path.Combine(Environment.SystemDirectory, "schtasks.exe"))
        { UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true };
        foreach (var argument in arguments) start.ArgumentList.Add(argument);
        using var process = Process.Start(start) ?? throw new IOException("Не удалось запустить Планировщик Windows.");
        var stdout = process.StandardOutput.ReadToEndAsync();
        var stderr = process.StandardError.ReadToEndAsync();
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(15));
        try { await process.WaitForExitAsync(timeout.Token); }
        catch { process.Kill(); throw; }
        await Task.WhenAll(stdout, stderr);
        return process.ExitCode;
    }
}
