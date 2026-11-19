using System.Windows;
using System.Windows.Automation.Peers;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Threading;
using DiscordRoute.App;
using static WindowsChecks;

internal static class QuietCheckTests
{
    public static void Run(MainWindow window, TabControl tabs, MainViewModel model, Action<bool, string> check, Action<string> capture)
    {
        tabs.SelectedIndex = 0;
        ((ScrollViewer)((TabItem)tabs.Items[0]).Content).ScrollToTop();
        CheckPrimaryAction(window, model, check);
        model.IsProcessRunning = true;
        model.IsVerified = true;
        check(model.Heading == "Связь включена", "Running connection has a stable heading independent of probe status");
        model.EnvironmentChanged();
        check(!model.IsVerified && model.Heading == "Связь включена" && !model.ShowProgress,
            "Network invalidation cannot claim that an idle connection is being checked");
        capture("connection-heading");
        CheckPrimaryAction(window, model, check);
        model.IsProcessRunning = false;
        model.Telegram.Recovery.Enable();
        check(model.Heading == "Ожидаем подключения", "Recovery intent without a process never claims an active connection or probe");
        model.Telegram.Recovery.Stop();
        model.IsVerified = true;
        var gate = new TaskCompletionSource();
        var quiet = model.RunAsync(ct => gate.Task.WaitAsync(ct), background: true);
        model.IsProcessRunning = true;
        window.UpdateLayout();
        check(model.AnyBusy && !model.ShowProgress && !((ProgressBar)window.FindName("ConnectionProgress")).IsVisible,
            "Background probe remains serialized without displaying progress");
        check(model.Heading == "Связь включена" && model.PrimaryActionLabel == "Отключить" && model.DiscordSummary.Contains("отвечает"),
            "Background probe preserves healthy heading, service summary and disconnect label");
        ((ScrollViewer)((TabItem)tabs.Items[0]).Content).ScrollToTop();
        capture("quiet-background-check");
        var overlaps = false;
        Finish(model.RunAsync(_ => { overlaps = true; return Task.CompletedTask; }));
        check(!overlaps && model.StopCommand.CanExecute(null), "Quiet probe rejects overlapping operations and allows disconnect");
        check(!model.FullScanCommand.CanExecute(null), "Full scan cannot overlap an active operation");
        gate.SetResult(); Finish(quiet);
        gate = new();
        var manual = model.RunAsync(ct => gate.Task.WaitAsync(ct));
        window.UpdateLayout();
        check(model.ShowProgress && model.Heading.Contains("Проверяем") && ((ProgressBar)window.FindName("ConnectionProgress")).IsVisible,
            "Manual operation still displays progress");
        CheckPrimaryAction(window, model, check);
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
        Finish(ConnectionOnlyChecks.RunAsync(model, check, () =>
        {
            tabs.SelectedIndex = 1;
            window.UpdateLayout();
            var open = (Button)window.FindName("TelegramOpenButton");
            var stop = (Button)window.FindName("TelegramAction");
            check(open.IsVisible && open.IsEnabled && open.Command == telegram.OpenCommand &&
                ((Button)window.FindName("TelegramCheckButton")).IsVisible && stop.Content.Equals("Остановить прокси"),
                "Running proxy exposes the Telegram link and server check beside the unchanged stop action");
            capture("telegram-running");
        }));
    }

    private static void CheckPrimaryAction(MainWindow window, MainViewModel model, Action<bool, string> check)
    {
        window.Dispatcher.Invoke(() => { }, DispatcherPriority.ApplicationIdle);
        var button = (Button)window.FindName("ConnectionAction");
        var label = Visuals<TextBlock>(button).Single(t => t.Text == model.PrimaryActionLabel);
        var peer = new ButtonAutomationPeer(button);
        check(button.Command == model.PrimaryCommand && button.IsTabStop && peer.IsKeyboardFocusable() && peer.GetName() == label.Text,
            "The primary action has a keyboard target and a matching screen reader name: " + label.Text);
        var center = label.TranslatePoint(new Point(label.ActualWidth / 2, label.ActualHeight / 2), window);
        var hit = window.InputHitTest(center);
        if (hit is ContentElement content) hit = ContentOperations.GetParent(content) as IInputElement;
        check(label.ActualWidth > 0 && label.ActualHeight > 0 && hit is Visual visual && (visual == button || button.IsAncestorOf(visual)),
            "Clicking the visible caption reaches the primary action: " + label.Text);
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
