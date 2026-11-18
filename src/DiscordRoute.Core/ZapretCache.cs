namespace DiscordRoute.Core;

public static class ZapretCache
{
    public static async Task ResetAsync(string dataRoot, AppState state, ConvenienceState preferences, Func<Task> removeDriver)
    {
        var settings = Path.Combine(dataRoot, "settings.json");
        state.ZapretResetPending = true;
        state.ActivePackage = state.PendingPackage = state.PreviousPackage = state.LastStrategy = null;
        state.VoiceServer = state.VoiceLogFingerprint = null;
        state.VoiceObservedAt = state.LastUpdateCheck = default;
        state.Save(settings);
        preferences.NetworkStrategies.Clear();
        preferences.Save(Path.Combine(dataRoot, "preferences.json"));

        await removeDriver();
        await Task.Run(() =>
        {
            foreach (var name in new[] { "Packages", "GameFilter" })
            {
                var path = Path.Combine(dataRoot, name);
                try { Directory.Delete(path, recursive: true); }
                catch (DirectoryNotFoundException) when (!File.Exists(path)) { }
            }
        });

        state.ZapretResetPending = false;
        try { state.Save(settings); }
        catch { state.ZapretResetPending = true; throw; }
    }
}
