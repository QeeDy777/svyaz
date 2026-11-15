using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal sealed class WindowsRunner(Action<string> log, Func<GameFilterSettings> gameFilter, string gameIpSetPath) : IStrategyRunner, IDisposable
{
    private Process? process;
    private SafeFileHandle? job;
    public bool IsRunning => process != null && !process.HasExited;

    public static void EnsureNoExternalProcess()
    {
        foreach (var name in new[] { "winws", "winws2" })
        {
            var processes = Process.GetProcessesByName(name);
            var any = processes.Length > 0;
            foreach (var item in processes) item.Dispose();
            if (any) throw new InvalidOperationException("Уже работает другой zapret (" + name + "). Остановите его перед подбором; приложение не будет завершать чужой процесс.");
        }
    }

    public async Task StartAsync(Package package, Strategy strategy, CancellationToken ct)
    {
        ct.ThrowIfCancellationRequested();
        await StopAsync();
        EnsureNoExternalProcess();
        var start = new ProcessStartInfo(Path.Combine(package.Root, "bin", "winws.exe"))
        {
            WorkingDirectory = Path.Combine(package.Root, "bin"), UseShellExecute = false,
            CreateNoWindow = true, RedirectStandardError = true, RedirectStandardOutput = true
        };
        foreach (var argument in Strategy.Parse(File.ReadAllText(strategy.FilePath), package.Root, gameFilter(), gameIpSetPath)) start.ArgumentList.Add(argument);
        job = CreateJobObject(IntPtr.Zero, null);
        if (job.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error());
        var limits = new JobLimits { BasicLimitInformation = new BasicLimits { LimitFlags = 0x2000 } };
        if (!SetInformationJobObject(job, 9, ref limits, (uint)Marshal.SizeOf<JobLimits>()))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        try
        {
            process = Process.Start(start) ?? throw new IOException("Не удалось запустить winws.");
            if (!AssignProcessToJobObject(job, process.Handle)) throw new Win32Exception(Marshal.GetLastWin32Error());
            process.ErrorDataReceived += (_, e) => { if (!string.IsNullOrWhiteSpace(e.Data)) log("winws: " + e.Data); };
            process.OutputDataReceived += (_, e) => { if (!string.IsNullOrWhiteSpace(e.Data)) log("winws: " + e.Data); };
            process.BeginErrorReadLine();
            process.BeginOutputReadLine();
            await Task.Delay(1200, ct);
            if (process.HasExited) throw new IOException("winws завершился, код " + process.ExitCode + ". См. журнал.");
        }
        catch { await StopAsync(); throw; }
    }

    public async Task StopAsync()
    {
        job?.Dispose();
        job = null;
        if (process == null) return;
        if (!process.HasExited)
        {
            try { process.Kill(entireProcessTree: true); }
            catch (InvalidOperationException) when (process.HasExited) { }
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(5));
            await process.WaitForExitAsync(timeout.Token);
        }
        process.Dispose();
        process = null;
    }

    public void Dispose()
    {
        job?.Dispose();
        process?.Dispose();
    }

    [StructLayout(LayoutKind.Sequential)] private struct BasicLimits
    {
        public long PerProcessUserTimeLimit, PerJobUserTimeLimit;
        public uint LimitFlags;
        public UIntPtr MinimumWorkingSetSize, MaximumWorkingSetSize;
        public uint ActiveProcessLimit;
        public UIntPtr Affinity;
        public uint PriorityClass, SchedulingClass;
    }
    [StructLayout(LayoutKind.Sequential)] private struct IoCounters
    {
        public ulong ReadOperationCount, WriteOperationCount, OtherOperationCount, ReadTransferCount, WriteTransferCount, OtherTransferCount;
    }
    [StructLayout(LayoutKind.Sequential)] private struct JobLimits
    {
        public BasicLimits BasicLimitInformation;
        public IoCounters IoInfo;
        public UIntPtr ProcessMemoryLimit, JobMemoryLimit, PeakProcessMemoryUsed, PeakJobMemoryUsed;
    }
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true, EntryPoint = "CreateJobObjectW")]
    private static extern SafeFileHandle CreateJobObject(IntPtr attributes, string? name);
    [DllImport("kernel32.dll", SetLastError = true)] [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetInformationJobObject(SafeFileHandle job, int infoClass, ref JobLimits info, uint length);
    [DllImport("kernel32.dll", SetLastError = true)] [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool AssignProcessToJobObject(SafeFileHandle job, IntPtr processHandle);
}
