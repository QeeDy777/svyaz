using System.ComponentModel;
using System.Windows;

namespace DiscordRoute.App;

public partial class App : Application
{
    private readonly Dictionary<string, object> palette = new();

    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);
        foreach (var key in new[] { "Paper", "Ink", "Muted", "Green", "Mint", "Line", "White", "Hover", "Error" })
            palette[key] = Resources["Brand" + key];
        ApplyContrast();
        SystemParameters.StaticPropertyChanged += OnSystemChanged;
    }

    private void OnSystemChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName == nameof(SystemParameters.HighContrast)) Dispatcher.Invoke(ApplyContrast);
    }

    private void ApplyContrast()
    {
        foreach (var (key, original) in palette)
            Resources["Brand" + key] = !SystemParameters.HighContrast ? original : key switch
            {
                "Paper" or "Mint" => SystemColors.WindowBrush,
                "White" => SystemColors.WindowBrush,
                "Ink" or "Green" or "Hover" => SystemColors.WindowTextBrush,
                _ => SystemColors.WindowTextBrush
            };
    }

    protected override void OnExit(ExitEventArgs e)
    {
        SystemParameters.StaticPropertyChanged -= OnSystemChanged;
        base.OnExit(e);
    }
}
