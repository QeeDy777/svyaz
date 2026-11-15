using System.Xml.Linq;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal static class InstallerMaintenance
{
    public static void Install()
    {
        var path = Autostart.InstallExecutable();
        var updater = new AppUpdater();
        var current = updater.Read("current");
        // A repair with an older installer must not downgrade an already updated app.
        if (current != null && AppUpdatePackage.ParseVersion(current.Version) > AppUpdater.RunningVersion)
        {
            updater.Executable(current);
            return;
        }
        updater.Promote(new(AppUpdater.RunningVersion.ToString(), Path.GetFileName(Path.GetDirectoryName(path)!)));
    }

    public static void Uninstall()
    {
        dynamic scheduler = Activator.CreateInstance(Type.GetTypeFromProgID("Schedule.Service")!)!;
        scheduler.Connect();
        dynamic folder = scheduler.GetFolder(@"\");
        var names = new List<string>();
        foreach (dynamic task in folder.GetTasks(1))
            if (OwnTask((string)task.Name, (string)task.Xml, Program.InstallRoot)) names.Add((string)task.Name);
        foreach (var name in names) folder.DeleteTask(name, 0);
    }

    internal static bool OwnTask(string name, string xml, string root)
    {
        if (!name.StartsWith("DiscordRoute-", StringComparison.Ordinal)) return false;
        XNamespace ns = "http://schemas.microsoft.com/windows/2004/02/mit/task";
        var commands = XDocument.Parse(xml).Descendants(ns + "Command").Select(x => x.Value).ToArray();
        var prefix = Path.GetFullPath(Path.Combine(root, "Versions")) + Path.DirectorySeparatorChar;
        return commands.Length == 1 && Path.GetFullPath(commands[0]).StartsWith(prefix, StringComparison.OrdinalIgnoreCase)
            && Path.GetFileName(commands[0]).Equals("Связь.exe", StringComparison.OrdinalIgnoreCase);
    }
}
