using DiscordRoute.Core;

internal static class ZapretCacheChecks
{
    internal static async Task RunAsync(Action<bool, string> check)
    {
        var root = Directory.CreateTempSubdirectory();
        try
        {
            var path = root.FullName;
            var package = root.CreateSubdirectory("Packages/obsolete/lists");
            File.WriteAllText(Path.Combine(package.FullName, "local-user.txt"), "old");
            File.WriteAllText(Path.Combine(path, "GameFilter"), "invalid directory");
            var telegram = root.CreateSubdirectory("Telegram/Embedded");
            var keep = Path.Combine(telegram.FullName, "fixture");
            File.WriteAllText(keep, "keep");
            var state = new AppState { ActivePackage = "active", PreviousPackage = "previous", PendingPackage = "pending",
                LastStrategy = "old", VoiceServer = "old.discord.media", AutoUpdate = false, AutostartConfigured = true,
                GameFilter = new() { Mode = GameFilterMode.Udp }, Telegram = new TelegramSettings().WithSecret() };
            var preferences = new ConvenienceState { Services = new() { Telegram = true }, QuietNotifications = false };
            preferences.NetworkStrategies.Add("network", "old");

            try { await ZapretCache.ResetAsync(path, state, preferences, () =>
                {
                    check(Directory.Exists(package.FullName), "Driver removal runs before package files are deleted");
                    return Task.CompletedTask;
                }); throw new Exception("Expected invalid cache directory failure"); }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException) { }
            var settings = Path.Combine(path, "settings.json");
            var resume = AppState.Load(settings);
            check(resume.ZapretResetPending && resume.ActivePackage == null, "Interrupted reset cannot reactivate a partially deleted package");
            File.Delete(Path.Combine(path, "GameFilter"));
            File.WriteAllText(Path.Combine(root.CreateSubdirectory("GameFilter").FullName, "ipset-all.txt"), "cached");
            var prefs = ConvenienceState.Load(Path.Combine(path, "preferences.json"));
            await ZapretCache.ResetAsync(path, resume, prefs, () => Task.CompletedTask);
            var clean = AppState.Load(settings);
            check(!clean.ZapretResetPending && clean.PreviousPackage == null && clean.PendingPackage == null &&
                clean.LastStrategy == null && clean.VoiceServer == null && prefs.NetworkStrategies.Count == 0 &&
                !Directory.Exists(Path.Combine(path, "Packages")) && !Directory.Exists(Path.Combine(path, "GameFilter")),
                "Reset removes package generations, cached lists and saved ALT choices");
            check(clean.Telegram.Secret == state.Telegram.Secret && File.Exists(keep) && clean.GameFilter.Mode == GameFilterMode.Udp &&
                !clean.AutoUpdate && clean.AutostartConfigured && !prefs.QuietNotifications && prefs.Services?.Telegram == true,
                "Reset preserves Telegram, Game Filter mode and unrelated preferences");
        }
        finally { root.Delete(recursive: true); }
    }
}
