using System.ComponentModel;
using System.Windows;
using System.Windows.Media;

namespace DiscordRoute.App;

public partial class App : Application
{
    private readonly Dictionary<string, Brush> palette = new();

    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);
        foreach (var key in Resources.Keys.OfType<string>())
            if (key.StartsWith("Brand", StringComparison.Ordinal) && Resources[key] is Brush brush)
                palette[key] = brush;
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
            Resources[key] = !SystemParameters.HighContrast ? original : key switch
            {
                "BrandPaper" or "BrandPanel" or "BrandPanel2" or "BrandInput" or "BrandButton"
                    or "BrandHero" or "BrandOnAccent" or "BrandAccentTint" => SystemColors.WindowBrush,
                _ => SystemColors.WindowTextBrush
            };
    }

    protected override void OnExit(ExitEventArgs e)
    {
        SystemParameters.StaticPropertyChanged -= OnSystemChanged;
        base.OnExit(e);
    }
}
