using System.Windows;
using System.Windows.Controls;
using System.Windows.Threading;
using DiscordRoute.App;

internal static class QuietCheckTests
{
    public static void Run(MainWindow window, TabControl tabs, MainViewModel model, Action<bool, string> check, Action<string> capture)
    {
        tabs.SelectedIndex = 0;
        model.IsVerified = true;
        var gate = new TaskCompletionSource();
        var quiet = model.RunAsync(ct => gate.Task.WaitAsync(ct), background: true);
        window.UpdateLayout();
        check(model.AnyBusy && !model.ShowProgress && !((ProgressBar)window.FindName("ConnectionProgress")).IsVisible,
            "Background probe remains serialized without displaying progress");
        check(model.Heading.Contains("отвечают") && model.PrimaryActionLabel == "Отключить" && model.DiscordSummary.Contains("отвечает"),
            "Background probe preserves healthy heading, service summary and disconnect label");
        ((ScrollViewer)((TabItem)tabs.Items[0]).Content).ScrollToTop();
        capture("quiet-background-check");
        var overlaps = false;
        Finish(model.RunAsync(_ => { overlaps = true; return Task.CompletedTask; }));
        check(!overlaps && model.StopCommand.CanExecute(null), "Quiet probe rejects overlapping operations and allows disconnect");
        gate.SetResult(); Finish(quiet);
        gate = new();
        var manual = model.RunAsync(ct => gate.Task.WaitAsync(ct));
        window.UpdateLayout();
        check(model.ShowProgress && model.Heading.Contains("Проверяем") && ((ProgressBar)window.FindName("ConnectionProgress")).IsVisible,
            "Manual operation still displays progress");
        gate.SetResult(); Finish(manual);
        gate = new();
        quiet = model.RunAsync(ct => gate.Task.WaitAsync(ct), background: true);
        model.StopCommand.Execute(null); Finish(quiet);
        check(!model.AnyBusy && !model.ShowDisconnect && !model.IsVerified, "Disconnect cancels a quiet probe and clears connection state");

        gate = new();
        var telegram = model.Telegram;
        quiet = telegram.RunAsync(ct => gate.Task.WaitAsync(ct), background: true);
        check(telegram.IsBusy && !telegram.ShowProgress && !model.ShowProgress && !telegram.Summary.Contains("операция") && telegram.PrimaryLabel != "Отменить",
            "Quiet Telegram operation does not flash Telegram or the common progress state");
        Finish(telegram.StopRequestedAsync()); Finish(quiet);
        check(!telegram.IsBusy && !telegram.Recovery.Wanted, "Telegram quiet operation cancels and completes before stopping");
        gate = new();
        manual = telegram.RunAsync(ct => gate.Task.WaitAsync(ct));
        check(telegram.ShowProgress && model.ShowProgress && telegram.PrimaryLabel == "Отменить", "Manual Telegram operation remains visible");
        gate.SetResult(); Finish(manual);
        telegram.Recovery.Enable();
        Finish(telegram.RunAsync(_ => throw new InvalidOperationException("quiet-check failure"), background: true));
        check(telegram.HasFailure && !telegram.ServerVerified && telegram.Recovery.Pending, "Background Telegram errors remain visible and trigger recovery");
        Finish(telegram.StopRequestedAsync());
        Finish(model.RunAsync(_ => throw new InvalidOperationException("quiet-check failure"), background: true));
        check(model.HasError && model.Heading.Contains("внимание"), "Background Discord errors remain visible");
        model.StopCommand.Execute(null);
    }

    private static void Finish(Task operation)
    {
        var frame = new DispatcherFrame();
        var timeout = DateTime.UtcNow.AddSeconds(10);
        var timer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(10) };
        timer.Tick += (_, _) => { if (operation.IsCompleted || DateTime.UtcNow >= timeout) frame.Continue = false; };
        timer.Start();
        try { if (!operation.IsCompleted) Dispatcher.PushFrame(frame); }
        finally { timer.Stop(); }
        if (!operation.IsCompleted) throw new TimeoutException("Quiet check did not complete");
        operation.GetAwaiter().GetResult();
    }
}
