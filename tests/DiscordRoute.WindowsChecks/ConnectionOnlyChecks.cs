using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Reflection;
using DiscordRoute.App;
using DiscordRoute.Core;

internal static class ConnectionOnlyChecks
{
    public static async Task RunAsync(MainViewModel model, Action<bool, string> check)
    {
        var state = Field<AppState>(model, "state");
        var autoUpdate = state.AutoUpdate;
        state.AutoUpdate = false;
        var runner = Field<WindowsRunner>(model, "runner");
        var process = Field<ManagedProcess>(runner, "process");
        var recovery = Field<ConnectionRecovery>(model, "recovery");
        var operations = 0;
        void Observe(object? sender, System.ComponentModel.PropertyChangedEventArgs args)
        {
            if (model.AnyBusy) operations++;
        }
        model.PropertyChanged += Observe;
        try
        {
            recovery.Enable();
            await model.RunAsync(_ => throw new ConnectionFailure("Подбор завершён", requiresManualRetry: true));
            model.EnvironmentChanged();
            await model.TickAsync();
            check(!recovery.Wanted && !recovery.Pending && model.HasError && model.PrimaryCommand == model.ConnectCommand,
                "A terminal selection error leaves the next connection to the user, even after a network event");
            process.Start(new ProcessStartInfo("powershell.exe", "-NoProfile -Command Start-Sleep -Seconds 120")
                { UseShellExecute = false, CreateNoWindow = true });
            model.IsProcessRunning = true;
            recovery.Enable();
            model.IsVerified = true;
            await model.TickAsync();
            check(operations == 0 && model.IsVerified && runner.IsRunning,
                "Connected timer starts no service probes or selection");
            model.EnvironmentChanged();
            check(!recovery.Pending && !model.IsVerified, "Network change invalidates results without scheduling checks for a running ALT");
            recovery.Invalidate(DateTimeOffset.UtcNow.AddMinutes(-1));
            for (var i = 0; i < 3; i++) await model.TickAsync();
            check(operations == 0 && recovery.Attempts == 0 && runner.IsRunning,
                "Even an overdue recovery cannot probe or replace a running ALT");

            var telegram = model.Telegram;
            var telegramRunner = Field<TelegramRunner>(telegram, "runner");
            using var listener = new TcpListener(IPAddress.Loopback, 0);
            listener.Start();
            var port = ((IPEndPoint)listener.LocalEndpoint).Port;
            listener.Stop();
            var settings = new TelegramSettings { Port = port }.WithSecret();
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(30));
            await telegramRunner.StartAsync(settings.Configuration("{\"pool_size\":0,\"cfproxy\":false}"), settings, timeout.Token);
            await telegram.RunAsync(_ => Task.CompletedTask);
            operations = 0;
            telegram.Recovery.Enable();
            var before = telegram.ServerCheck;
            await telegram.TickAsync();
            check(operations == 0 && ReferenceEquals(before, telegram.ServerCheck) && telegramRunner.IsRunning,
                "Connected Telegram timer starts no MTProto probe");
            telegram.NetworkChanged(DateTimeOffset.UtcNow);
            check(!telegram.Recovery.Pending, "Telegram network change does not schedule a running-engine check");
            telegram.Recovery.Invalidate(DateTimeOffset.UtcNow.AddMinutes(-1));
            before = telegram.ServerCheck;
            await telegram.TickAsync();
            check(operations == 0 && telegram.Recovery.Attempts == 0 && ReferenceEquals(before, telegram.ServerCheck) && telegramRunner.IsRunning,
                "Overdue Telegram recovery cannot probe or restart a running engine");
        }
        finally
        {
            model.PropertyChanged -= Observe;
            recovery.Stop();
            await runner.StopAsync();
            await model.Telegram.StopRequestedAsync();
            model.IsProcessRunning = false;
            state.AutoUpdate = autoUpdate;
        }
    }

    private static T Field<T>(object instance, string name) =>
        (T)(instance.GetType().GetField(name, BindingFlags.Instance | BindingFlags.NonPublic)?.GetValue(instance)
            ?? throw new InvalidOperationException("Missing fixture field: " + name));
}
