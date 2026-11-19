using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Text;

namespace DiscordRoute.App;

internal static class ShortcutIcons
{
    private const string LauncherName = "Связь.exe";
    private const string ShortcutName = "Связь.lnk";
    private static readonly Guid ShellLinkClass = new("00021401-0000-0000-C000-000000000046");

    internal static void Refresh(string installRoot, string executable)
    {
        foreach (var folder in new[] { Environment.SpecialFolder.CommonPrograms, Environment.SpecialFolder.CommonDesktopDirectory })
        {
            var directory = Environment.GetFolderPath(folder, Environment.SpecialFolderOption.DoNotVerify);
            if (directory.Length != 0) RefreshLink(Path.Combine(directory, ShortcutName), installRoot, executable);
        }
    }

    internal static bool RefreshLink(string shortcutPath, string installRoot, string executable)
    {
        string? temporary = null;
        try
        {
            if (!LocalPath(shortcutPath) || !LocalPath(installRoot) || !LocalPath(executable) ||
                !Same(Path.GetFileName(shortcutPath), ShortcutName) ||
                !ManagedExecutable(installRoot, executable) || !File.Exists(executable) ||
                !File.Exists(shortcutPath) || HasReparsePoint(executable) || HasReparsePoint(shortcutPath) ||
                !Same(Path.GetPathRoot(installRoot), Path.GetPathRoot(shortcutPath))) return false;

            var stable = Path.Combine(installRoot, "Application", LauncherName);
            var instance = Activator.CreateInstance(Type.GetTypeFromCLSID(ShellLinkClass, throwOnError: true)!)!;
            try
            {
                var link = (IShellLinkW)instance;
                var persist = (IPersistFile)instance;
                persist.Load(shortcutPath, 0); // STGM_READ
                var path = new StringBuilder(32768);
                link.GetPath(path, path.Capacity, 0, 4); // SLGP_RAWPATH, without resolving the link
                if (path.Length == 0)
                {
                    link.GetIDList(out var idList);
                    try
                    {
                        if (idList == 0 || !SHGetPathFromIDListEx(idList, path, (uint)path.Capacity, 0)) return false;
                    }
                    finally { Marshal.FreeCoTaskMem(idList); }
                }
                if (!Same(path.ToString(), stable)) return false;
                path.Clear();
                link.GetIconLocation(path, path.Capacity, out var index);
                var oldIcon = path.ToString();
                if (index != 0 || Same(oldIcon, executable) ||
                    oldIcon.Length != 0 && !ManagedExecutable(installRoot, oldIcon)) return false;

                // Save a copy under Program Files. Explorer keeps the original link if serialization fails.
                temporary = Path.Combine(installRoot, Guid.NewGuid().ToString("N") + ".lnk");
                link.SetIconLocation(executable, 0);
                persist.Save(temporary, false);
            }
            finally { Marshal.FinalReleaseComObject(instance); }

            File.Replace(temporary, shortcutPath, null);
            SHChangeNotify(0x2000, 0x0005, shortcutPath, 0); // SHCNE_UPDATEITEM, SHCNF_PATHW
            return true;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or COMException or
            ArgumentException or NotSupportedException or System.Security.SecurityException) { return false; }
        finally
        {
            if (temporary != null)
                try { File.Delete(temporary); } catch (IOException) { } catch (UnauthorizedAccessException) { }
        }
    }

    private static bool ManagedExecutable(string root, string path)
    {
        if (Same(path, Path.Combine(root, "Application", LauncherName))) return true;
        var id = Path.GetFileName(Path.GetDirectoryName(path));
        return id is { Length: 64 } && id.All(char.IsAsciiHexDigit) &&
            Same(path, Path.Combine(root, "Versions", id, LauncherName));
    }

    private static bool Same(string? a, string? b) => string.Equals(a, b, StringComparison.OrdinalIgnoreCase);

    private static bool LocalPath(string path) => Path.IsPathFullyQualified(path) && path.Length > 2 &&
        char.IsAsciiLetter(path[0]) && path[1] == ':' && Same(Path.GetFullPath(path), path);

    private static bool HasReparsePoint(string path)
    {
        for (string? current = path; current != null; current = Path.GetDirectoryName(current))
            if ((File.GetAttributes(current) & FileAttributes.ReparsePoint) != 0) return true;
        return false;
    }

    [DllImport("shell32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
    private static extern void SHChangeNotify(int eventId, uint flags, string path, nint unused);

    [DllImport("shell32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SHGetPathFromIDListEx(nint idList, [Out] StringBuilder path, uint length, uint flags);

    [ComImport, Guid("000214F9-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IShellLinkW
    {
        void GetPath([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int length, nint findData, int flags);
        void GetIDList(out nint idList); void SetIDList();
        void GetDescription(); void SetDescription();
        void GetWorkingDirectory(); void SetWorkingDirectory();
        void GetArguments(); void SetArguments();
        void GetHotkey(); void SetHotkey();
        void GetShowCmd(); void SetShowCmd();
        void GetIconLocation([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int length, out int index);
        void SetIconLocation([MarshalAs(UnmanagedType.LPWStr)] string path, int index);
    }
}
