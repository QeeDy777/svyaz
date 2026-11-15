using DiscordRoute.Core;

internal static class ServiceSelectionChecks
{
    public static void Run(Action<bool, string> check)
    {
        var defaults = new ServiceSelection();
        check(defaults.Discord && defaults.YouTube && !defaults.Telegram, "Default selection preserves Discord and YouTube");
        var skipped = new CheckResult(false, "Не выбрано", true);
        var ok = new CheckResult(true, "OK");
        var fail = new CheckResult(false, "timeout");
        check(new ProbeResult(skipped, skipped, skipped, new(ok, ok)).Success, "YouTube alone does not require Discord");
        check(!new ProbeResult(skipped, skipped, skipped, new(ok, fail)).Success, "Selected YouTube failure is mandatory");
        check(new ProbeResult(ok, ok, ok, new(skipped, skipped)).Success, "Discord alone does not require YouTube");
        check(!new ProbeResult(skipped, skipped, skipped, new(skipped, skipped)).Success, "No checks is never a verified connection");
        try { new ServiceSelection { Discord = false, YouTube = false }.Validate(); throw new Exception("empty accepted"); }
        catch (InvalidDataException) { check(true, "Empty service selection rejected"); }
        var old = System.Text.Json.JsonSerializer.Deserialize<ConvenienceState>("{\"LastSeenAppVersion\":\"0.8.0\"}")!;
        check(old.Services == null && old.QuietNotifications, "Old preferences retain migration marker and quiet alerts");
    }
}
