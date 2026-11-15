using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace DiscordRoute.App;

internal sealed class ManagedProcess : IDisposable
{
    private Process? process;
    private SafeFileHandle? job;
    public int? ExitCode => process?.HasExited == true ? process.ExitCode : null;
    public bool IsRunning => process != null && !process.HasExited;

    public void Start(ProcessStartInfo start, Action<string>? log = null)
    {
        if (process != null) throw new InvalidOperationException("Сначала остановите управляемый процесс.");
        try
        {
            job = CreateJobObject(IntPtr.Zero, null);
            if (job.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error());
            var limits = new JobLimits { BasicLimitInformation = new BasicLimits { LimitFlags = 0x2000 } };
            if (!SetInformationJobObject(job, 9, ref limits, (uint)Marshal.SizeOf<JobLimits>()))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            process = new Process { StartInfo = start };
            if (log != null)
            {
                process.ErrorDataReceived += (_, e) => { if (!string.IsNullOrWhiteSpace(e.Data)) log(e.Data); };
                process.OutputDataReceived += (_, e) => { if (!string.IsNullOrWhiteSpace(e.Data)) log(e.Data); };
            }
            if (!process.Start()) throw new IOException("Не удалось запустить компонент.");
            if (!AssignProcessToJobObject(job, process.Handle)) throw new Win32Exception(Marshal.GetLastWin32Error());
            if (log != null) { process.BeginErrorReadLine(); process.BeginOutputReadLine(); }
        }
        catch { Dispose(); throw; }
    }

    public async Task StopAsync()
    {
        job?.Dispose(); job = null;
        if (process == null) return;
        if (!process.HasExited)
        {
            try { process.Kill(entireProcessTree: true); }
            catch (InvalidOperationException) when (process.HasExited) { }
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(5));
            await process.WaitForExitAsync(timeout.Token);
        }
        process.Dispose(); process = null;
    }

    public void Dispose()
    {
        job?.Dispose(); job = null;
        if (process != null)
        {
            try { if (!process.HasExited) process.Kill(entireProcessTree: true); }
            catch (InvalidOperationException) { }
            catch (Win32Exception) { }
            process.Dispose(); process = null;
        }
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
