using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using DiscordRoute.App;
using static WindowsChecks;

internal static class UpdateNoticeChecks
{
    public static void Run(MainWindow window, TabControl tabs, Action<bool, string> check, Action<string> capture)
    {
        var updates = ((MainViewModel)window.DataContext).AppUpdates;
        var notices = 0;
        updates.UpdateReady += _ => notices++;
        check(!updates.IsNoticeVisible && !updates.RestartCommand.CanExecute(null), "No update notice before download");
        var version = AppUpdater.RunningVersion;
        var next = new InstalledApp(new Version(version.Major, version.Minor, version.Build + 1).ToString(), new string('a', 64));
        using (var restored = new AppUpdateViewModel(new() { AutoUpdate = false }, _ => { }, _ => { }))
        {
            restored.SetPending(next);
            var restoredNotices = 0;
            restored.UpdateReady += _ => restoredNotices++;
            restored.AnnouncePending();
            restored.AnnouncePending();
            check(restoredNotices == 1 && restored.IsNoticeVisible, "Saved update is announced after the window subscribes, even with automatic updates disabled");
        }
        updates.SetPending(next);
        check(updates.IsNoticeVisible && notices == 1 && updates.RestartCommand.CanExecute(null), "Downloaded update shows one actionable notice");
        updates.SettingsChanged();
        updates.SetPending(next);
        check(notices == 1, "Repeated check and settings change do not repeat update notification");
        tabs.SelectedIndex = 0;
        capture("update-ready");
        var notice = (Border)window.FindName("UpdateNotice");
        check(notice.IsVisible && Visuals<Button>(notice).Any(b => b.Command == updates.RestartCommand), "Banner uses the validated restart command");
        tabs.SelectedIndex = 2;
        window.UpdateLayout();
        check(notice.IsVisible, "Update notice remains visible across tabs");
        updates.DismissCommand.Execute(null);
        updates.SetPending(next);
        check(!updates.IsNoticeVisible && updates.RestartCommand.CanExecute(null) && notices == 1, "Later hides notice without deleting the prepared update or repeating it");
        updates.SetPending(next with { Id = new string('b', 64) });
        check(updates.IsNoticeVisible && notices == 2, "A different downloaded package is announced again");
        updates.RestartCommand.Execute(null);
        check(DiscordRoute.App.Program.Restart == null && updates.NoticeDetail.Contains("Versions"), "Missing runtime reports failure in banner instead of restarting");
        updates.SetPending(new(AppUpdater.RunningVersion.ToString(), next.Id));
        check(!updates.IsNoticeVisible && !updates.RestartCommand.CanExecute(null), "Installed or older version is never offered for restart");

        window.Dispatcher.Invoke(() => { }, System.Windows.Threading.DispatcherPriority.ApplicationIdle);
        var scroll = (ScrollViewer)((TabItem)tabs.Items[2]).Content;
        var presenter = (ScrollContentPresenter)scroll.Template.FindName("PART_ScrollContentPresenter", scroll);
        var bar = (ScrollBar)scroll.Template.FindName("PART_VerticalScrollBar", scroll);
        var edge = presenter.TranslatePoint(new Point(presenter.ActualWidth, 0), scroll).X;
        check(bar.TranslatePoint(new Point(0, 0), scroll).X - edge >= 14, "Scrollbar reserves at least 14 DIP between text and hit area");
        check(scroll.ScrollableHeight > 0 && bar.IsVisible, "Long settings remain scrollable");
        ScrollBar.PageDownCommand.Execute(null, bar);
        window.Dispatcher.Invoke(() => { }, System.Windows.Threading.DispatcherPriority.ApplicationIdle);
        check(scroll.VerticalOffset > 0, "Track click scrolls the settings page");
        scroll.ScrollToTop();
        window.Dispatcher.Invoke(() => { }, System.Windows.Threading.DispatcherPriority.ApplicationIdle);
        var thumb = Visuals<Thumb>(bar).Single();
        thumb.RaiseEvent(new DragDeltaEventArgs(0, 20) { RoutedEvent = Thumb.DragDeltaEvent });
        window.Dispatcher.Invoke(() => { }, System.Windows.Threading.DispatcherPriority.ApplicationIdle);
        check(scroll.VerticalOffset > 0, "Dragging the custom thumb scrolls content");
        scroll.ScrollToBottom();
        window.Dispatcher.Invoke(() => { }, System.Windows.Threading.DispatcherPriority.ApplicationIdle);
        check(Math.Abs(scroll.VerticalOffset - scroll.ScrollableHeight) < 1, "Last settings remain reachable");
        capture("settings-bottom");

        tabs.SelectedIndex = 3;
        window.UpdateLayout();
        var journal = Visuals<TextBox>(window).Single(t => t.IsReadOnly);
        var hint = (FrameworkElement)window.FindName("EmptyJournalHint");
        journal.Text = "";
        window.Dispatcher.Invoke(() => { }, System.Windows.Threading.DispatcherPriority.ApplicationIdle);
        check(hint.IsVisible && !hint.IsHitTestVisible, "An empty journal shows a hint without intercepting clicks");
        journal.Text = string.Join('\n', Enumerable.Repeat("Строка журнала — проверка прокрутки", 200));
        window.Dispatcher.Invoke(() => { }, System.Windows.Threading.DispatcherPriority.ApplicationIdle);
        check(!hint.IsVisible, "The hint clears when journal entries appear");
        journal.ScrollToEnd();
        window.Dispatcher.Invoke(() => { }, System.Windows.Threading.DispatcherPriority.ApplicationIdle);
        check(journal.VerticalOffset > 0 && journal.SelectedText == "", "Journal remains scrollable and selectable with the shared gutter");
        journal.SelectAll();
        check(journal.SelectedText.Length > 100, "Journal text selection remains available");
        updates.ExitAsync().GetAwaiter().GetResult();
        updates.SetPending(next);
        check(notices == 2 && !updates.IsNoticeVisible, "No late notifications during exit");
    }
}
