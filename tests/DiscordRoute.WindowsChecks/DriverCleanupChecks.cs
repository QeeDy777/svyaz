using System.IO;
using System.Diagnostics;
using DiscordRoute.App;
using DiscordRoute.Core;

internal static class DriverCleanupChecks
{
    public static async Task RunAsync(Action<bool, string> check)
    {
        var name = "WinDivert" + DateTime.UtcNow.Ticks.ToString(System.Globalization.CultureInfo.InvariantCulture);
        var root = Directory.CreateTempSubdirectory();
        var sys = Path.Combine(root.FullName, "bin", "WinDivert64.sys");
        try
        {
            check(WinDivertCleanup.OwnsDriver(@"\??\" + sys, root.FullName) &&
                !WinDivertCleanup.OwnsDriver(Path.Combine(root.FullName + "-other", "WinDivert64.sys"), root.FullName),
                "Driver paths allow NT prefixes and exclude sibling installations");
            if (await ScAsync("create", name, "type=", "kernel", "start=", "demand", "binPath=", sys) != 0)
                throw new IOException("Could not register isolated driver service fixture");
            try
            {
                try
                {
                    await WinDivertCleanup.RemoveServiceAsync(name, Path.Combine(root.FullName, "different"), default);
                    throw new Exception("Foreign driver was accepted");
                }
                catch (ConnectionFailure) { }
                check(await ScAsync("query", name) == 0, "Foreign service remains registered");
                var managed = Path.Combine(root.FullName, "different");
                check(!WinDivertCleanup.FindLegacyDrivers(managed).ContainsKey(name), "Unknown third-party service is excluded from reset");
                var deleted = Path.Combine(root.FullName, "zapret-discord-youtube-1.0", "bin", "WinDivert64.sys");
                check(WinDivertCleanup.IsLegacyZapret(deleted) && !WinDivertCleanup.IsLegacyZapret(sys),
                    "Missing files are recognized only for a conventional zapret path");
                Directory.CreateDirectory(Path.GetDirectoryName(sys)!);
                File.WriteAllText(Path.Combine(Path.GetDirectoryName(sys)!, "winws.exe"), "");
                File.WriteAllText(Path.Combine(root.FullName, "service.bat"), "");
                var approved = WinDivertCleanup.FindLegacyDrivers(managed);
                check(approved.TryGetValue(name, out var selected) && selected.Equals(sys, StringComparison.OrdinalIgnoreCase),
                    "Reset confirmation identifies the actual registered legacy path");
                foreach (var permission in new string?[] { null, deleted })
                {
                    try
                    {
                        await WinDivertCleanup.RemoveServiceAsync(name, managed, default, permission);
                        throw new Exception("Legacy driver removed without matching approval");
                    }
                    catch (ConnectionFailure) { }
                }
                await WinDivertCleanup.RemoveServiceAsync(name, managed, default, approved[name]);
                check(await ScAsync("query", name) == 1060, "SCM confirms the approved legacy driver registration is gone");
                if (await ScAsync("create", name, "type=", "kernel", "start=", "demand", "binPath=", sys) != 0)
                    throw new IOException("Could not recreate managed driver service fixture");
                await WinDivertCleanup.RemoveServiceAsync(name, root.FullName, default);
                check(await ScAsync("query", name) == 1060, "App-managed driver can still be removed without legacy approval");
                await WinDivertCleanup.RemoveServiceAsync(name, root.FullName, default);
            }
            finally { await ScAsync("delete", name); }
        }
        finally { root.Delete(true); }
    }

    private static async Task<int> ScAsync(params string[] args)
    {
        var info = new ProcessStartInfo(Path.Combine(Environment.SystemDirectory, "sc.exe")) { UseShellExecute = false };
        foreach (var arg in args) info.ArgumentList.Add(arg);
        using var process = Process.Start(info)!;
        await process.WaitForExitAsync();
        return process.ExitCode;
    }
}
