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

    private static string[] DriverServices()
    {
        using var services = Registry.LocalMachine.OpenSubKey(ServicesKey)
            ?? throw new IOException("Не удалось прочитать список служб Windows.");
        return services.GetSubKeyNames().Where(name =>
            name.StartsWith("WinDivert", StringComparison.OrdinalIgnoreCase) &&
            name["WinDivert".Length..].All(c => char.IsAsciiDigit(c) || c == '.'))
            .Append("WinDivert").Append("WinDivert14").Distinct(StringComparer.OrdinalIgnoreCase).ToArray();
    }

    internal static Dictionary<string, string> FindLegacyDrivers(string packages)
    {
        var result = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        foreach (var name in DriverServices())
        {
            using var key = Registry.LocalMachine.OpenSubKey(ServicesKey + "\\" + name);
            if (key?.GetValue("Type") is 1 && DriverPath(key.GetValue("ImagePath") as string) is { } path &&
                !OwnsDriver(path, packages) && IsLegacyZapret(path))
                result.Add(name, path);
        }
        return result;
    }

    internal static async Task RemoveAsync(string packages, Action<string> log, CancellationToken ct,
        IReadOnlyDictionary<string, string>? approvedLegacy = null)
    {
        await WindowsRunner.EnsureNoExternalProcessAsync(ct);
        foreach (var name in DriverServices())
        {
            var approved = approvedLegacy?.GetValueOrDefault(name);
            await RemoveServiceAsync(name, packages, ct, approved);
            if (approved != null) log($"Проверено удаление службы старого zapret {name} ({approved}).");
        }

        using var sources = Registry.LocalMachine.OpenSubKey(ServicesKey + @"\EventLog\System", writable: true);
        using (var source = sources?.OpenSubKey("WinDivert"))
        {
            var path = DriverPath(source?.GetValue("EventMessageFile") as string);
            if (OwnsDriver(path, packages) || path != null && approvedLegacy?.Values.Contains(path, StringComparer.OrdinalIgnoreCase) == true)
                sources!.DeleteSubKey("WinDivert", throwOnMissingSubKey: false);
        }
        log("Службы WinDivert проверены. Можно удалить файлы прежнего пакета.");
    }

    private static string? DriverPath(string? image)
    {
        if (string.IsNullOrWhiteSpace(image)) return null;
        var path = image.Trim().Trim('"');
        if (path.StartsWith(@"\??\", StringComparison.Ordinal) || path.StartsWith(@"\\?\", StringComparison.Ordinal))
            path = path[4..];
        if (!Path.IsPathFullyQualified(path) || path.Length < 3 || !char.IsAsciiLetter(path[0]) ||
            path[1] != ':' || path[2] != Path.DirectorySeparatorChar) return null;
        var file = Path.GetFileName(path);
        return file.Equals("WinDivert64.sys", StringComparison.OrdinalIgnoreCase) ||
            file.Equals("WinDivert32.sys", StringComparison.OrdinalIgnoreCase) ||
            file.Equals("WinDivert.sys", StringComparison.OrdinalIgnoreCase) ? Path.GetFullPath(path) : null;
    }

    internal static bool OwnsDriver(string? image, string packages) =>
        DriverPath(image)?.StartsWith(Path.TrimEndingDirectorySeparator(Path.GetFullPath(packages)) +
            Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase) == true;

    internal static bool IsLegacyZapret(string image)
    {
        if (DriverPath(image) is not { } path) return false;
        var bin = Directory.GetParent(path);
        if (bin?.Parent is not { } root || !bin.Name.Equals("bin", StringComparison.OrdinalIgnoreCase)) return false;
        if ((File.Exists(Path.Combine(bin.FullName, "winws.exe")) || File.Exists(Path.Combine(bin.FullName, "winws2.exe"))) &&
            File.Exists(Path.Combine(root.FullName, "service.bat"))) return true;

        // A removed distribution can leave only its service ImagePath in Windows.
        var knownFolder = root.Name.Equals("zapret", StringComparison.OrdinalIgnoreCase) ||
            root.Name.Equals("zapret-discord-youtube", StringComparison.OrdinalIgnoreCase) ||
            root.Name.StartsWith("zapret-discord-youtube-", StringComparison.OrdinalIgnoreCase);
        if (!knownFolder) return false;
        try { _ = File.GetAttributes(path); }
        catch (Exception ex) when (ex is FileNotFoundException or DirectoryNotFoundException) { return true; }
        return false;
    }

    internal static async Task RemoveServiceAsync(string name, string packages, CancellationToken ct, string? approvedLegacy = null)
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
                    var image = DriverPath(key?.GetValue("ImagePath") as string);
                    if (key?.GetValue("Type") is not 1 || image == null || !(OwnsDriver(image, packages) ||
                        image.Equals(approvedLegacy, StringComparison.OrdinalIgnoreCase) && IsLegacyZapret(image)))
                        throw new ConnectionFailure($"Служба {name} установлена вне пакетов «Связи». Для старого zapret запустите сброс вручную и проверьте путь в окне подтверждения. Для другого драйвера используйте программу, которая его установила.", requiresManualRetry: true);

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
