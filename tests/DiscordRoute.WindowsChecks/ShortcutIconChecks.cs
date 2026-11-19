using System.IO;
using System.Runtime.InteropServices;
using DiscordRoute.App;

internal static class ShortcutIconChecks
{
    public static void Run(Action<bool, string> check)
    {
        var root = Directory.CreateTempSubdirectory("Связь shortcut ").FullName;
        dynamic shell = Activator.CreateInstance(Type.GetTypeFromProgID("WScript.Shell")!)!;
        try
        {
            var launcher = MakeExecutable(Path.Combine(root, "Application"));
            var previous = MakeExecutable(Path.Combine(root, "Versions", new string('a', 64)));
            var updated = MakeExecutable(Path.Combine(root, "Versions", new string('b', 64)));
            var foreign = MakeExecutable(Path.Combine(root, "Another app"));
            var shortcut = Path.Combine(root, "Связь.lnk");
            check(!ShortcutIcons.RefreshLink(shortcut, root, updated) && !File.Exists(shortcut), "A deleted shortcut is not recreated");

            void Create(string target, string? icon = null)
            {
                File.Delete(shortcut);
                dynamic link = shell.CreateShortcut(shortcut);
                try
                {
                    link.TargetPath = target;
                    link.Arguments = "--startup";
                    link.WorkingDirectory = root;
                    link.Description = "Моя связь";
                    link.WindowStyle = 3;
                    if (icon != null) link.IconLocation = icon;
                    link.Save();
                }
                finally { Marshal.FinalReleaseComObject((object)link); }
            }

            void Verify(string expectedIcon)
            {
                dynamic link = shell.CreateShortcut(shortcut);
                try
                {
                    var location = (string)link.IconLocation;
                    var separator = location.LastIndexOf(',');
                    check(separator > 0 && location[(separator + 1)..].Trim() == "0" &&
                        string.Equals(location[..separator].Trim().Trim('"'), expectedIcon, StringComparison.OrdinalIgnoreCase),
                        "Explorer sees the confirmed version's icon");
                    check(string.Equals((string)link.TargetPath, launcher, StringComparison.OrdinalIgnoreCase) &&
                        (string)link.Arguments == "--startup" && string.Equals((string)link.WorkingDirectory, root, StringComparison.OrdinalIgnoreCase) &&
                        (string)link.Description == "Моя связь" && (int)link.WindowStyle == 3,
                        "Refreshing the icon preserves the stable target, arguments, directory, description and window style");
                }
                finally { Marshal.FinalReleaseComObject((object)link); }
            }

            Create(launcher);
            check(ShortcutIcons.RefreshLink(shortcut, root, updated), "An existing shortcut using the launcher's default icon is migrated");
            Verify(updated);
            var earlier = new DateTime(2020, 1, 2, 3, 4, 5, DateTimeKind.Utc);
            File.SetLastWriteTimeUtc(shortcut, earlier);
            check(!ShortcutIcons.RefreshLink(shortcut, root, updated) && File.GetLastWriteTimeUtc(shortcut) == earlier,
                "A repeated refresh leaves an up-to-date shortcut untouched");
            check(ShortcutIcons.RefreshLink(shortcut, root, previous), "Rollback restores the previous release's icon");
            Verify(previous);

            Create(launcher, launcher + ",0");
            check(!ShortcutIcons.RefreshLink(shortcut, root, foreign), "A file outside the app's runtime directories cannot become the shortcut icon");
            Verify(launcher);
            check(ShortcutIcons.RefreshLink(shortcut, root, updated), "A shortcut explicitly using the stable launcher icon also migrates");
            Verify(updated);

            foreach (var (target, icon) in new[] { (foreign, launcher + ",0"), (launcher, foreign + ",0"), (launcher, previous + ",1") })
            {
                Create(target, icon);
                var before = File.ReadAllBytes(shortcut);
                check(!ShortcutIcons.RefreshLink(shortcut, root, updated) && before.SequenceEqual(File.ReadAllBytes(shortcut)),
                    "Custom targets, icon files and non-default icon indexes are preserved");
            }
        }
        finally
        {
            Marshal.FinalReleaseComObject((object)shell);
            Directory.Delete(root, true);
        }
    }

    private static string MakeExecutable(string folder)
    {
        Directory.CreateDirectory(folder);
        var path = Path.Combine(folder, "Связь.exe");
        File.Copy(Path.Combine(Environment.SystemDirectory, "cmd.exe"), path);
        return path;
    }
}
