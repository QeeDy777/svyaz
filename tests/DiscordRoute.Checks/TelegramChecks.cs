using System.Text.Json;
using DiscordRoute.Core;

internal static class TelegramChecks
{
    public static void Run(Action<bool, string> check)
    {
        var settings = new TelegramSettings().WithSecret();
        check(!settings.StartWithApp && settings.Port == 1443 && settings.Secret.Length == 32, "Telegram is opt-in and generates a 128-bit local key");
        check(settings.WithSecret().Secret == settings.Secret && new TelegramSettings().WithSecret().Secret != settings.Secret, "Telegram key survives restart and differs between installations");
        check(settings.ProxyUrl() == $"tg://proxy?server=127.0.0.1&port=1443&secret=dd{settings.Secret}", "Telegram URL matches upstream padded MTProto format");
        using var json = JsonDocument.Parse(settings.Configuration("{\"host\":\"0.0.0.0\",\"autostart\":true,\"check_updates\":true,\"pool_size\":2}"));
        check(json.RootElement.GetProperty("host").GetString() == "127.0.0.1" && !json.RootElement.GetProperty("autostart").GetBoolean() && !json.RootElement.GetProperty("check_updates").GetBoolean(), "Managed Telegram config cannot expose a LAN listener or independent startup/updater");
        check(json.RootElement.GetProperty("pool_size").GetInt32() == 2, "Telegram keeps upstream advanced settings");
        foreach (var bad in new[] { settings with { Port = 0 }, settings with { Port = 65536 }, settings with { Secret = "abc&server=evil" }, settings with { Secret = new string('f', 32) + "\n" } })
        {
            try { bad.Validate(); throw new Exception("Invalid Telegram settings accepted"); }
            catch (InvalidDataException) { check(true, "Reject malformed Telegram port/key"); }
        }
        check(JsonSerializer.Deserialize<AppState>("{}")!.Telegram.Secret == "", "Old settings migrate without starting Telegram");
        var root = Path.Combine(Path.GetTempPath(), "svyaz-telegram-" + Guid.NewGuid().ToString("N"));
        try
        {
            settings.WriteConfiguration(Path.Combine(root, "old"));
            (settings with { Port = 1444 }).WriteConfiguration(Path.Combine(root, "new"), Path.Combine(root, "old"));
            using var saved = JsonDocument.Parse(File.ReadAllText(TelegramSettings.ConfigurationPath(Path.Combine(root, "new"))));
            check(saved.RootElement.GetProperty("secret").GetString() == settings.Secret && saved.RootElement.GetProperty("port").GetInt32() == 1444, "Telegram update preserves the key and applies the new port");
            File.WriteAllText(TelegramSettings.ConfigurationPath(Path.Combine(root, "old")), settings.Configuration("{\"pool_size\":3}"));
            settings.WriteConfiguration(Path.Combine(root, "new"), Path.Combine(root, "old"));
            using var retried = JsonDocument.Parse(File.ReadAllText(TelegramSettings.ConfigurationPath(Path.Combine(root, "new"))));
            check(retried.RootElement.GetProperty("pool_size").GetInt32() == 3, "Retry update copies the latest active advanced settings, not stale pending config");
            check(File.Exists(Path.Combine(root, "new", "TgWsProxy_data", ".first_run_done_mtproto")), "Managed Telegram package suppresses duplicate onboarding");
        }
        finally { Directory.Delete(root, true); }
    }
}
