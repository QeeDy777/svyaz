using System.Text.Json;

namespace DiscordRoute.Core;

// Deliberately excludes free-form messages, addresses, paths and settings.
public sealed record DiagnosticReport(
    Version AppVersion, Version WindowsVersion, bool DiscordProcessRunning, bool DiscordVerified,
    bool TelegramProcessRunning, bool? ApiPassed, bool? GatewayPassed, bool? VoicePassed,
    DateTimeOffset? LastSuccessfulCheck, int TestedStrategies, int? LastDiscordError, int? LastTelegramError,
    bool? YouTubeSitePassed, bool? YouTubeGoogleVideoPassed)
{
    public string ToJson() => JsonSerializer.Serialize(this, new JsonSerializerOptions { WriteIndented = true });
}
