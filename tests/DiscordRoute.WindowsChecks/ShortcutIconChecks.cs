using System.IO;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Text;
using DiscordRoute.App;

internal static class ShortcutIconChecks
{
    public static void Run(Action<bool, string> check)
    {
        var root = Directory.CreateTempSubdirectory("Связь shortcut ").FullName;
        try
        {
            var launcher = MakeExecutable(Path.Combine(root, "Application"));
            var previous = MakeExecutable(Path.Combine(root, "Versions", new string('a', 64)));
            var updated = MakeExecutable(Path.Combine(root, "Versions", new string('b', 64)));
            var foreign = MakeExecutable(Path.Combine(root, "Another app"));
            var shortcut = Path.Combine(root, "Связь.lnk");
            check(!ShortcutIcons.RefreshLink(shortcut, root, updated) && !File.Exists(shortcut), "A deleted shortcut is not recreated");

            void Create(string target, string? icon = null, int index = 0)
            {
                File.Delete(shortcut);
                var link = NewLink();
                try
                {
                    link.SetPath(target);
                    link.SetArguments("--startup");
                    link.SetWorkingDirectory(root);
                    link.SetDescription("Моя связь");
                    link.SetShowCmd(3);
                    if (icon != null) link.SetIconLocation(icon, index);
                    ((IPersistFile)link).Save(shortcut, true);
                }
                finally { Marshal.FinalReleaseComObject(link); }
            }

            void Verify(string expectedIcon)
            {
                var link = NewLink();
                try
                {
                    ((IPersistFile)link).Load(shortcut, 0);
                    var icon = new StringBuilder(32768);
                    link.GetIconLocation(icon, icon.Capacity, out var index);
                    check(index == 0 && string.Equals(icon.ToString(), expectedIcon, StringComparison.OrdinalIgnoreCase),
                        "Explorer sees the confirmed version's icon");
                    var target = new StringBuilder(32768);
                    var arguments = new StringBuilder(256);
                    var directory = new StringBuilder(32768);
                    var description = new StringBuilder(256);
                    link.GetPath(target, target.Capacity, 0, 0);
                    link.GetArguments(arguments, arguments.Capacity);
                    link.GetWorkingDirectory(directory, directory.Capacity);
                    link.GetDescription(description, description.Capacity);
                    link.GetShowCmd(out var showCmd);
                    check(string.Equals(target.ToString(), launcher, StringComparison.OrdinalIgnoreCase) &&
                        arguments.ToString() == "--startup" && string.Equals(directory.ToString(), root, StringComparison.OrdinalIgnoreCase) &&
                        description.ToString() == "Моя связь" && showCmd == 3,
                        "Refreshing the icon preserves the stable target, arguments, directory, description and window style");
                }
                finally { Marshal.FinalReleaseComObject(link); }
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

            Create(launcher, launcher);
            check(!ShortcutIcons.RefreshLink(shortcut, root, foreign), "A file outside the app's runtime directories cannot become the shortcut icon");
            Verify(launcher);
            check(ShortcutIcons.RefreshLink(shortcut, root, updated), "A shortcut explicitly using the stable launcher icon also migrates");
            Verify(updated);

            foreach (var (target, icon, index) in new[] { (foreign, launcher, 0), (launcher, foreign, 0), (launcher, previous, 1) })
            {
                Create(target, icon, index);
                var before = File.ReadAllBytes(shortcut);
                check(!ShortcutIcons.RefreshLink(shortcut, root, updated) && before.SequenceEqual(File.ReadAllBytes(shortcut)),
                    "Custom targets, icon files and non-default icon indexes are preserved");
            }
        }
        finally { Directory.Delete(root, true); }
    }

    private static string MakeExecutable(string folder)
    {
        Directory.CreateDirectory(folder);
        var path = Path.Combine(folder, "Связь.exe");
        File.Copy(Path.Combine(Environment.SystemDirectory, "cmd.exe"), path);
        return path;
    }

    private static IShellLinkW NewLink() => (IShellLinkW)Activator.CreateInstance(
        Type.GetTypeFromCLSID(new Guid("00021401-0000-0000-C000-000000000046"), throwOnError: true)!)!;

    [ComImport, Guid("000214F9-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IShellLinkW
    {
        void GetPath([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int length, nint findData, uint flags);
        void GetIDList(out nint idList);
        void SetIDList(nint idList);
        void GetDescription([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder text, int length);
        void SetDescription([MarshalAs(UnmanagedType.LPWStr)] string text);
        void GetWorkingDirectory([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int length);
        void SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string path);
        void GetArguments([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder text, int length);
        void SetArguments([MarshalAs(UnmanagedType.LPWStr)] string text);
        void GetHotkey(out ushort key);
        void SetHotkey(ushort key);
        void GetShowCmd(out int showCmd);
        void SetShowCmd(int showCmd);
        void GetIconLocation([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int length, out int index);
        void SetIconLocation([MarshalAs(UnmanagedType.LPWStr)] string path, int index);
        void SetRelativePath([MarshalAs(UnmanagedType.LPWStr)] string path, uint reserved);
        void Resolve(nint window, uint flags);
        void SetPath([MarshalAs(UnmanagedType.LPWStr)] string path);
    }
}
