using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using DiscordRoute.Core;
using Microsoft.Win32;
using Microsoft.Win32.SafeHandles;

namespace DiscordRoute.App;

internal static class WinDivertCleanup
{
    private const string ServicesKey = @"SYSTEM\CurrentControlSet\Services";
    private const int DoesNotExist = 1060, CannotAcceptControl = 1061, NotActive = 1062, MarkedForDelete = 1072;
    private const uint ManagerConnect = 1, ServiceStop = 0x20, Delete = 0x10000;

    internal static async Task RemoveAsync(string packages, Action<string> log, CancellationToken ct)
    {
        using var services = Registry.LocalMachine.OpenSubKey(ServicesKey)
            ?? throw new IOException("Не удалось прочитать список служб Windows.");
        var names = services.GetSubKeyNames().Where(name =>
            name.StartsWith("WinDivert", StringComparison.OrdinalIgnoreCase) &&
            name["WinDivert".Length..].All(c => char.IsAsciiDigit(c) || c == '.'));
        foreach (var name in names.Append("WinDivert").Append("WinDivert14").Distinct(StringComparer.OrdinalIgnoreCase))
            await RemoveServiceAsync(name, packages, ct);

        using var sources = Registry.LocalMachine.OpenSubKey(ServicesKey + @"\EventLog\System", writable: true);
        using (var source = sources?.OpenSubKey("WinDivert"))
        {
            if (OwnsDriver(source?.GetValue("EventMessageFile") as string, packages))
                sources!.DeleteSubKey("WinDivert", throwOnMissingSubKey: false);
        }
        log("Службы WinDivert проверены. Можно удалить файлы прежнего пакета.");
    }

    internal static bool OwnsDriver(string? image, string packages)
    {
        if (string.IsNullOrWhiteSpace(image)) return false;
        var path = image.Trim().Trim('"');
        if (path.StartsWith(@"\??\", StringComparison.Ordinal) || path.StartsWith(@"\\?\", StringComparison.Ordinal))
            path = path[4..];
        if (!Path.IsPathFullyQualified(path)) return false;
        var file = Path.GetFileName(path);
        return (file.Equals("WinDivert64.sys", StringComparison.OrdinalIgnoreCase) ||
                file.Equals("WinDivert32.sys", StringComparison.OrdinalIgnoreCase) ||
                file.Equals("WinDivert.sys", StringComparison.OrdinalIgnoreCase)) &&
            Path.GetFullPath(path).StartsWith(Path.TrimEndingDirectorySeparator(Path.GetFullPath(packages)) +
                Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase);
    }

    internal static async Task RemoveServiceAsync(string name, string packages, CancellationToken ct)
    {
        using var manager = OpenSCManagerW(null, null, ManagerConnect);
        if (manager.IsInvalid) throw new Win32Exception(Marshal.GetLastPInvokeError());
        var watch = Stopwatch.StartNew();
        while (watch.Elapsed < TimeSpan.FromSeconds(12))
        {
            ct.ThrowIfCancellationRequested();
            using (var service = OpenServiceW(manager, name, ServiceStop | Delete))
            {
                if (service.IsInvalid)
                {
                    var error = Marshal.GetLastPInvokeError();
                    if (error == DoesNotExist) return;
                    if (error != MarkedForDelete) throw Failure(name, error);
                }
                else
                {
                    using var key = Registry.LocalMachine.OpenSubKey(ServicesKey + "\\" + name);
                    if (key?.GetValue("Type") is not 1 || !OwnsDriver(key.GetValue("ImagePath") as string, packages))
                        throw new ConnectionFailure($"Служба {name} установлена вне пакетов «Связи». Удалите её через программу, которая установила драйвер, и повторите сброс.", requiresManualRetry: true);

                    var stopped = ControlService(service, 1, out _) ? 0 : Marshal.GetLastPInvokeError();
                    if (stopped is not (0 or NotActive or CannotAcceptControl or MarkedForDelete)) throw Failure(name, stopped);
                    if (stopped is (0 or NotActive) && !DeleteService(service))
                    {
                        var error = Marshal.GetLastPInvokeError();
                        if (error != MarkedForDelete) throw Failure(name, error);
                    }
                }
            }
            await Task.Delay(250, ct);
        }
        throw new ConnectionFailure($"Windows ещё не выгрузила {name}. Закройте программы с WinDivert или перезагрузите ПК и повторите сброс.", requiresManualRetry: true);
    }

    private static ConnectionFailure Failure(string name, int code) =>
        new($"Не удалось удалить службу {name}. Код Windows {code} — {new Win32Exception(code).Message}", requiresManualRetry: true);

    private sealed class ServiceHandle() : SafeHandleZeroOrMinusOneIsInvalid(true)
    {
        protected override bool ReleaseHandle() => CloseServiceHandle(handle);
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ServiceStatus { public uint Type, State, Controls, ExitCode, SpecificExitCode, CheckPoint, WaitHint; }

    [DllImport("advapi32.dll", ExactSpelling = true, CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern ServiceHandle OpenSCManagerW(string? machine, string? database, uint access);
    [DllImport("advapi32.dll", ExactSpelling = true, CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern ServiceHandle OpenServiceW(ServiceHandle manager, string name, uint access);
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool ControlService(ServiceHandle service, uint control, out ServiceStatus status);
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool DeleteService(ServiceHandle service);
    [DllImport("advapi32.dll")]
    private static extern bool CloseServiceHandle(nint handle);
}
