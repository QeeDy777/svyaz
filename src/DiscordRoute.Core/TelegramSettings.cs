using System.Security.Cryptography;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;

namespace DiscordRoute.Core;

public sealed record TelegramSettings
{
    public int Port { get; init; } = 1443;
    public string Secret { get; init; } = "";
    public bool StartWithApp { get; init; }

    public TelegramSettings Validate()
    {
        if (Port is < 1 or > 65535) throw new InvalidDataException("Порт Telegram должен быть от 1 до 65535.");
        if (Secret == null || (Secret.Length != 0 && !Regex.IsMatch(Secret, @"\A[0-9a-fA-F]{32}\z")))
            throw new InvalidDataException("Повреждён ключ локального прокси Telegram.");
        return this with { Secret = Secret.ToLowerInvariant() };
    }

    public TelegramSettings WithSecret()
    {
        var value = Validate();
        return value.Secret.Length == 0 ? value with { Secret = Convert.ToHexStringLower(RandomNumberGenerator.GetBytes(16)) } : value;
    }

    public string ProxyUrl()
    {
        var value = Validate();
        if (value.Secret.Length == 0) throw new InvalidDataException("Сначала запустите прокси Telegram.");
        return $"tg://proxy?server=127.0.0.1&port={value.Port}&secret=dd{value.Secret}";
    }

    public string Configuration(string? previous = null)
    {
        var value = Validate();
        if (value.Secret.Length == 0) throw new InvalidDataException("Не создан ключ Telegram.");
        if (previous?.Length > 65536) throw new InvalidDataException("Настройки Telegram слишком велики.");
        var config = previous == null ? new JsonObject() : JsonNode.Parse(previous) as JsonObject
            ?? throw new InvalidDataException("Неверный формат настроек Telegram.");
        config["host"] = "127.0.0.1";
        config["port"] = value.Port;
        config["secret"] = value.Secret;
        config["autostart"] = false;
        config["check_updates"] = false;
        config["language"] = "ru";
        return config.ToJsonString();
    }

    public static string ConfigurationPath(string packageRoot) => Path.Combine(packageRoot, "TgWsProxy_data", "config.json");

    public void WriteConfiguration(string packageRoot, string? previousRoot = null)
    {
        var path = ConfigurationPath(packageRoot);
        var previous = previousRoot == null ? null : ConfigurationPath(previousRoot);
        var source = previous != null && File.Exists(previous) ? previous : path;
        if (source != null && File.Exists(source) && new FileInfo(source).Length > 65536)
            throw new InvalidDataException("Настройки Telegram слишком велики.");
        var content = Configuration(source != null && File.Exists(source) ? File.ReadAllText(source) : null);
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path + ".tmp", content);
        File.Move(path + ".tmp", path, true);
        // Connection instructions live in Svyaz; do not show the upstream first-run dialog again.
        File.WriteAllText(Path.Combine(Path.GetDirectoryName(path)!, ".first_run_done_mtproto"), "");
    }
}
