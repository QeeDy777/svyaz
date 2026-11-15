using DiscordRoute.Core;

internal static class ConvenienceChecks
{
    public static void Run(Action<bool, string> check)
    {
        var report = new DiagnosticReport(new Version(0, 7, 0), new Version(10, 0, 26100), true, false, true,
            true, false, null, null, 3, unchecked((int)0x80004005), null, true, false).ToJson();
        using var json = System.Text.Json.JsonDocument.Parse(report);
        var fields = json.RootElement.EnumerateObject().Select(p => p.Name).ToArray();
        check(fields.SequenceEqual(new[] { "AppVersion", "WindowsVersion", "DiscordProcessRunning", "DiscordVerified", "TelegramProcessRunning", "ApiPassed", "GatewayPassed", "VoicePassed", "LastSuccessfulCheck", "TestedStrategies", "LastDiscordError", "LastTelegramError", "YouTubeSitePassed", "YouTubeGoogleVideoPassed" }), "Diagnostic export has only reviewed safe fields");
        check(json.RootElement.GetProperty("VoicePassed").ValueKind == System.Text.Json.JsonValueKind.Null && !json.RootElement.GetProperty("GatewayPassed").GetBoolean(), "Diagnostic export distinguishes failed and untested checks");
        var home = Guid.NewGuid(); var mobile = Guid.NewGuid();
        var homeId = NetworkProfiles.Identity([home]); var mobileId = NetworkProfiles.Identity([mobile]);
        check(homeId != mobileId && homeId?.Length == 64, "Network profiles distinguish networks without storing names");
        check(NetworkProfiles.Identity([home, mobile, home]) == NetworkProfiles.Identity([mobile, home]), "Network identity is stable across order and duplicates");
        check(NetworkProfiles.Identity([]) == null, "Disconnected network has no profile");
        var state = new ConvenienceState();
        check(NetworkProfiles.Remember(state, homeId, homeId, "ALT1") && NetworkProfiles.Remember(state, mobileId, mobileId, "ALT2"), "Successful strategies are stored per network");
        check(state.NetworkStrategies[homeId!] == "ALT1" && state.NetworkStrategies[mobileId!] == "ALT2", "Home and mobile strategies do not overwrite each other");
        check(!NetworkProfiles.Remember(state, homeId, mobileId, "wrong") && state.NetworkStrategies[homeId!] == "ALT1", "Network change during probe cannot poison profile");
        check(!NetworkProfiles.Remember(state, null, null, "wrong"), "Unknown network is never memorized");
        for (var i = 0; i < 40; i++) NetworkProfiles.Remember(state, i.ToString(), i.ToString(), "ALT");
        check(state.NetworkStrategies.Count == 32, "Network profile storage is bounded");
    }
}
