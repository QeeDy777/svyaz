using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal sealed class TelegramRunner(Action<string>? log = null) : ITelegramRunner, IDisposable
{
    private readonly ManagedProcess process = new();
    private string? extractionDirectory;
    internal static string TempRoot => Path.Combine(Program.DataRoot, "Telegram", "Temp");
    public bool IsRunning => process.IsRunning;

    public async Task StartAsync(Package package, TelegramSettings settings, CancellationToken ct)
    {
        ct.ThrowIfCancellationRequested();
        settings.Validate();
        await StopAsync();
        if (Mutex.TryOpenExisting(@"Local\TgWsProxy_SingleInstance", out var external))
        {
            external.Dispose();
            throw new IOException("Уже запущен отдельный TG WS Proxy. Завершите его через его трей; чужой процесс не будет остановлен.");
        }
        var listener = new TcpListener(IPAddress.Loopback, settings.Port);
        try { listener.Server.ExclusiveAddressUse = true; listener.Start(); }
        catch (SocketException ex) { throw new IOException("Порт Telegram занят или недоступен. Выберите другой порт в настройках.", ex); }
        finally { listener.Stop(); }
        TelegramPackageStore.Verify(package);
        Directory.CreateDirectory(TempRoot);
        foreach (var stale in Directory.EnumerateDirectories(TempRoot))
            if (Guid.TryParseExact(Path.GetFileName(stale), "N", out _)) Cleanup(stale);
        var temp = extractionDirectory = Path.Combine(TempRoot, Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(temp);
        var start = new ProcessStartInfo(Path.Combine(package.Root, TelegramPackageStore.Executable))
        { WorkingDirectory = package.Root, UseShellExecute = false, CreateNoWindow = true };
        start.ArgumentList.Add("--portable");
        // Keep PyInstaller extraction in the protected app directory instead of a user-writable temp directory.
        start.Environment["TEMP"] = temp;
        start.Environment["TMP"] = temp;
        try
        {
            ct.ThrowIfCancellationRequested();
            process.Start(start);
            for (var attempt = 0; attempt < 120; attempt++)
            {
                await Task.Delay(500, ct);
                if (!IsRunning) throw new IOException("TG WS Proxy завершился при запуске. Проверьте журнал компонента.");
                using var client = new TcpClient(AddressFamily.InterNetwork);
                try
                {
                    using var probe = CancellationTokenSource.CreateLinkedTokenSource(ct);
                    probe.CancelAfter(500);
                    await client.ConnectAsync(IPAddress.Loopback, settings.Port, probe.Token);
                    if (IsRunning) return;
                }
                catch (SocketException) { }
                catch (OperationCanceledException) when (!ct.IsCancellationRequested) { }
            }
            throw new IOException("TG WS Proxy не открыл локальный порт за отведённое время.");
        }
        catch { await StopAsync(); throw; }
    }

    public async Task StopAsync()
    {
        await process.StopAsync();
        if (extractionDirectory == null) return;
        for (var attempt = 0; attempt < 10; attempt++)
        {
            if (Cleanup(extractionDirectory)) { extractionDirectory = null; return; }
            await Task.Delay(200);
        }
        log?.Invoke("Telegram: временные файлы заняты; очистка повторится при следующем запуске.");
    }

    private static bool Cleanup(string path)
    {
        try { if (Directory.Exists(path)) Directory.Delete(path, true); return true; }
        catch (IOException) { return false; }
        catch (UnauthorizedAccessException) { return false; }
    }

    public void Dispose()
    {
        process.Dispose();
        if (extractionDirectory != null) Cleanup(extractionDirectory);
    }
}
