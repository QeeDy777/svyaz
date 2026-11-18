using System.IO;
using System.Diagnostics;
using DiscordRoute.App;
using DiscordRoute.Core;

internal static class DriverCleanupChecks
{
    public static async Task RunAsync(Action<bool, string> check)
    {
        var name = "SvyazFixture" + Guid.NewGuid().ToString("N");
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
                await WinDivertCleanup.RemoveServiceAsync(name, root.FullName, default);
                check(await ScAsync("query", name) == 1060, "SCM confirms managed driver registration is gone");
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
