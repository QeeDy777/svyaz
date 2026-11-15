using System.Net;
using System.Net.Sockets;
using System.Text.Json;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal sealed class TelegramRunner : IDisposable
{
    private readonly SemaphoreSlim gate = new(1);
    private bool initialized;
    private bool disposed;
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web);
    private sealed record EngineState(bool Alive, bool Ready, string? Error);
    private EngineState State => initialized
        ? JsonSerializer.Deserialize<EngineState>(TelegramPython.Call("status")!, Json) ?? throw new IOException("Нет состояния движка Telegram.")
        : new(false, false, null);
    public bool IsRunning => State.Alive;

    public async Task StartAsync(string configuration, TelegramSettings settings, CancellationToken ct)
    {
        await gate.WaitAsync(ct).ConfigureAwait(false);
        try
        {
            ObjectDisposedException.ThrowIf(disposed, this);
            settings.Validate();
            await StopCoreAsync().ConfigureAwait(false);
            if (Mutex.TryOpenExisting(@"Local\TgWsProxy_SingleInstance", out var external))
            {
                external.Dispose();
                throw new IOException("Уже запущен отдельный TG WS Proxy. Завершите его через его трей.");
            }
            var listener = new TcpListener(IPAddress.Loopback, settings.Port);
            try { listener.Server.ExclusiveAddressUse = true; listener.Start(); }
            catch (SocketException ex) { throw new IOException("Порт Telegram занят или недоступен. Выберите другой порт.", ex); }
            finally { listener.Stop(); }
            await Task.Run(() =>
            {
                ct.ThrowIfCancellationRequested();
                var root = TelegramRuntime.Prepare(Path.Combine(Program.DataRoot, "Telegram", "Embedded"));
                TelegramPython.Initialize(root);
                initialized = true;
            }, ct).ConfigureAwait(false);
            ct.ThrowIfCancellationRequested();
            TelegramPython.Call("start", configuration);
            try
            {
                for (var attempt = 0; attempt < 150; attempt++)
                {
                    ct.ThrowIfCancellationRequested();
                    var status = State;
                    if (status.Error != null || !status.Alive)
                        throw new IOException("Встроенный Telegram не запустился: " + (status.Error ?? "движок остановился") + ".");
                    if (status.Ready) return;
                    await Task.Delay(100, ct).ConfigureAwait(false);
                }
                throw new IOException("Встроенный Telegram не открыл локальный порт за отведённое время.");
            }
            catch { await StopCoreAsync().ConfigureAwait(false); throw; }
        }
        finally { gate.Release(); }
    }

    public async Task StopAsync()
    {
        await gate.WaitAsync().ConfigureAwait(false);
        try { await StopCoreAsync().ConfigureAwait(false); }
        finally { gate.Release(); }
    }

    private async Task StopCoreAsync()
    {
        if (!initialized) return;
        TelegramPython.Call("stop");
        for (var attempt = 0; attempt < 200; attempt++)
        {
            if (!State.Alive) return;
            await Task.Delay(50).ConfigureAwait(false);
        }
        throw new IOException("Движок Telegram не завершился. Перезапустите «Связь» перед повторным подключением.");
    }

    public void Dispose()
    {
        disposed = true;
        if (initialized) TelegramPython.Call("stop");
    }
}
