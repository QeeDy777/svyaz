using System.Text;
using System.Text.RegularExpressions;

namespace DiscordRoute.Core;

public sealed record Strategy(string Name, string FilePath)
{
    public static IReadOnlyList<string> Parse(string script, string packageRoot, GameFilterSettings? gameFilter = null, string? gameIpSetPath = null)
    {
        var settings = gameFilter ?? new GameFilterSettings();
        var ranges = settings.Ranges();
        var start = Regex.Match(script, "(?im)^start[^\\r\\n]*?\"%BIN%winws\\.exe\"[ \\t]+", RegexOptions.CultureInvariant);
        if (!start.Success) throw new InvalidDataException("Неподдерживаемый формат стратегии: нет команды winws.");
        var command = Regex.Replace(script[(start.Index + start.Length)..], @"\^\s*\r?\n", " ").Replace("^!", "!", StringComparison.Ordinal).Trim();
        if (command.IndexOfAny(['\r', '\n', '&', '|', '<', '>', '`', '^']) >= 0)
            throw new InvalidDataException("Стратегия содержит неподдерживаемые команды.");

        var tokens = new List<string>();
        var token = new StringBuilder();
        var quoted = false;
        foreach (var character in command)
        {
            if (character == '"') quoted = !quoted;
            else if (char.IsWhiteSpace(character) && !quoted)
            {
                if (token.Length > 0) { tokens.Add(token.ToString()); token.Clear(); }
            }
            else token.Append(character);
        }
        if (quoted) throw new InvalidDataException("Незакрытая кавычка в стратегии.");
        if (token.Length > 0) tokens.Add(token.ToString());
        if (tokens.Count == 0 || tokens.Any(t => !t.StartsWith("--", StringComparison.Ordinal)))
            throw new InvalidDataException("Ожидались только параметры winws.");

        return tokens.Select(t => Regex.Replace(t, @"%(?<name>[^%]+)%", match =>
        {
            var name = match.Groups["name"].Value.ToUpperInvariant();
            return name switch
            {
                "BIN" => Path.Combine(packageRoot, "bin") + Path.DirectorySeparatorChar,
                "LISTS" => Path.Combine(packageRoot, "lists") + Path.DirectorySeparatorChar,
                "GAMEFILTER" => ranges.Legacy,
                "GAMEFILTERTCP" => ranges.Tcp,
                "GAMEFILTERUDP" => ranges.Udp,
                _ => throw new InvalidDataException("Неизвестная переменная стратегии: " + name)
            };
        })).Select(t =>
        {
            if (t.Contains('%') || t.Contains('\0')) throw new InvalidDataException("Неподдерживаемый параметр.");
            var value = t.Split('=', 2).ElementAtOrDefault(1);
            if (value != null && (value.Contains("../") || value.Contains("..\\")))
                throw new InvalidDataException("Выход пути за папку пакета.");
            if (settings.Mode != GameFilterMode.Disabled && gameIpSetPath != null &&
                t.Equals("--ipset=" + Path.Combine(packageRoot, "lists", "ipset-all.txt"), StringComparison.OrdinalIgnoreCase))
                return "--ipset=" + Path.GetFullPath(gameIpSetPath);
            return t;
        }).ToArray();
    }

    public static IReadOnlyList<Strategy> Load(string root, Action<string> log)
    {
        var result = new List<Strategy>();
        foreach (var path in Directory.GetFiles(root, "general*.bat").Order(StringComparer.OrdinalIgnoreCase))
        {
            try { Parse(File.ReadAllText(path), root); result.Add(new(Path.GetFileNameWithoutExtension(path), path)); }
            catch (InvalidDataException ex) { log(Path.GetFileName(path) + ": " + ex.Message); }
        }
        if (result.Count == 0) throw new InvalidDataException("В пакете нет совместимых стратегий.");
        return result;
    }
}
