using System.Xml.Linq;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal static class InstallerMaintenance
{
    public static void Install()
    {
        var updater = new AppUpdater();
        var current = updater.Read("current");
        string path;
        // A repair with an older installer must not downgrade an already updated app.
        if (current != null && AppUpdatePackage.ParseVersion(current.Version) > AppUpdater.RunningVersion)
            path = updater.Executable(current);
        else
        {
            path = RepairRuntime();
            updater.Promote(new(AppUpdater.RunningVersion.ToString(), Path.GetFileName(Path.GetDirectoryName(path)!)));
        }
        VisitTasks((folder, task) =>
        {
            dynamic definition = task.Definition;
            definition.Actions.Item(1).Path = path;
            folder.RegisterTaskDefinition(task.Name, definition, 4, definition.Principal.UserId, null, definition.Principal.LogonType);
        });
    }

    private static string RepairRuntime()
    {
        var source = Environment.ProcessPath!;
        var id = RuntimeInstallation.Identity(Path.GetDirectoryName(source)!);
        var destination = Path.Combine(Program.InstallRoot, "Versions", id);
        var damaged = false;
        if (Directory.Exists(destination))
            try { damaged = RuntimeInstallation.Identity(destination) != id; }
            catch (IOException) { damaged = true; }
        var backup = destination + ".repair-" + Guid.NewGuid().ToString("N");
        if (damaged) Directory.Move(destination, backup);
        try
        {
            var installed = RuntimeInstallation.Install(source, Program.InstallRoot);
            if (damaged) Directory.Delete(backup, true);
            return installed;
        }
        catch
        {
            if (damaged && !Directory.Exists(destination)) Directory.Move(backup, destination);
            throw;
        }
    }

    public static void Uninstall() => VisitTasks((folder, task) => folder.DeleteTask(task.Name, 0));

    private static void VisitTasks(Action<dynamic, dynamic> visit)
    {
        dynamic scheduler = Activator.CreateInstance(Type.GetTypeFromProgID("Schedule.Service")!)!;
        scheduler.Connect();
        dynamic folder = scheduler.GetFolder(@"\");
        var tasks = new List<dynamic>();
        foreach (dynamic task in folder.GetTasks(1))
            if (OwnTask((string)task.Name, (string)task.Xml, Program.InstallRoot)) tasks.Add(task);
        foreach (var task in tasks) visit(folder, task);
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
