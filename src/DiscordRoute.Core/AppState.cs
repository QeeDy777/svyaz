using System.Text.Json;

namespace DiscordRoute.Core;

public sealed class AppState
{
    public bool ZapretResetPending { get; set; }
    public string? ActivePackage { get; set; }
    public string? PendingPackage { get; set; }
    public string? PreviousPackage { get; set; }
    public string? LastStrategy { get; set; }
    public string? VoiceServer { get; set; }
    public string? VoiceLogFingerprint { get; set; }
    public DateTimeOffset VoiceObservedAt { get; set; }
    public bool AutoUpdate { get; set; } = true;
    public GameFilterSettings GameFilter { get; set; } = new();
    public TelegramSettings Telegram { get; set; } = new();
    public string? TelegramActivePackage { get; set; }
    public string? TelegramPendingPackage { get; set; }
    public bool AutostartConfigured { get; set; }
    public DateTimeOffset LastUpdateCheck { get; set; }

    public static AppState Load(string path) => File.Exists(path)
        ? JsonSerializer.Deserialize<AppState>(File.ReadAllText(path)) ?? throw new InvalidDataException("Настройки пусты.")
        : new();

    public void Save(string path)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        var temp = path + ".tmp";
        File.WriteAllText(temp, JsonSerializer.Serialize(this, new JsonSerializerOptions { WriteIndented = true }));
        File.Move(temp, path, true);
    }
}
