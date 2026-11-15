using System.Text.Json;
using DiscordRoute.Core;

internal static class TelegramMigrationChecks
{
    internal static void Run(Action<bool, string> check)
    {
        var root = Path.Combine(Path.GetTempPath(), "svyaz-telegram-migration-" + Guid.NewGuid().ToString("N"));
        try
        {
            var id = new string('a', 32);
            var old = Path.Combine(root, "Packages", id);
            var settings = new TelegramSettings { Port = 9443 }.WithSecret();
            Directory.CreateDirectory(Path.GetDirectoryName(TelegramSettings.ConfigurationPath(old))!);
            var original = "{\"dc_ip\":[\"4:149.154.167.220\"],\"pool_size\":8,\"host\":\"0.0.0.0\"}";
            File.WriteAllText(TelegramSettings.ConfigurationPath(old), original);
            var migrated = settings.EmbeddedConfiguration(root, id, null);
            using (var data = JsonDocument.Parse(migrated))
            {
                check(data.RootElement.GetProperty("pool_size").GetInt32() == 8 && data.RootElement.GetProperty("dc_ip").GetArrayLength() == 1,
                    "Embedded Telegram preserves advanced settings from the active legacy component");
                check(data.RootElement.GetProperty("port").GetInt32() == 9443 && data.RootElement.GetProperty("secret").GetString() == settings.Secret &&
                    data.RootElement.GetProperty("host").GetString() == "127.0.0.1", "Embedded migration preserves proxy link and enforces loopback");
            }
            check(File.ReadAllText(TelegramSettings.ConfigurationPath(old)) == original, "Legacy configuration stays intact for app rollback");
            File.WriteAllText(TelegramSettings.ConfigurationPath(old), "invalid legacy JSON");
            check(settings.EmbeddedConfiguration(root, id, null) == migrated, "Subsequent startup uses the migrated settings without reimporting legacy changes");
            Directory.Delete(root, true);
            try { settings.EmbeddedConfiguration(root, "../../other", null); throw new Exception("Traversal accepted"); }
            catch (InvalidDataException) { check(true, "Legacy Telegram identifiers cannot escape package directory"); }
            Directory.CreateDirectory(Path.GetDirectoryName(TelegramSettings.ConfigurationPath(root))!);
            File.WriteAllText(TelegramSettings.ConfigurationPath(root), new string('x', 65537));
            try { settings.EmbeddedConfiguration(root, null, null); throw new Exception("Oversized config accepted"); }
            catch (InvalidDataException) { check(true, "Embedded Telegram rejects oversized configuration without replacing it"); }
        }
        finally { if (Directory.Exists(root)) Directory.Delete(root, true); }
    }
}
