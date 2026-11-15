using System.Text.Json;

namespace DiscordRoute.Core;

// Old releases rewrite settings.json; keep new preferences out of that file for rollback.
public sealed class ConvenienceState
{
    public Dictionary<string, string> NetworkStrategies { get; set; } = new();
    public ServiceSelection? Services { get; set; }
    public bool QuietNotifications { get; set; } = true;
    public string? LastSeenAppVersion { get; set; }

    public static ConvenienceState Load(string path)
    {
        var value = File.Exists(path) ? JsonSerializer.Deserialize<ConvenienceState>(File.ReadAllText(path)) : null;
        value ??= new();
        value.NetworkStrategies ??= new();
        return value;
    }

    public void Save(string path)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path + ".tmp", JsonSerializer.Serialize(this, new JsonSerializerOptions { WriteIndented = true }));
        File.Move(path + ".tmp", path, true);
    }
}
