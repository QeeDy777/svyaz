using System.Globalization;
using System.Text.RegularExpressions;

namespace DiscordRoute.Core;

public enum GameFilterMode { Disabled, All, Tcp, Udp }

public sealed record GameFilterSettings
{
    public GameFilterMode Mode { get; init; }
    public string TcpPorts { get; init; } = "1024-65535";
    public string UdpPorts { get; init; } = "1024-65535";

    public GameFilterSettings Validate()
    {
        if (!Enum.IsDefined(Mode)) throw new InvalidDataException("Неизвестный режим Game Filter.");
        return this with { TcpPorts = ValidatePorts(TcpPorts), UdpPorts = ValidatePorts(UdpPorts) };
    }

    public (string Legacy, string Tcp, string Udp) Ranges()
    {
        var value = Validate();
        var tcp = Mode is GameFilterMode.All or GameFilterMode.Tcp ? value.TcpPorts : "12";
        var udp = Mode is GameFilterMode.All or GameFilterMode.Udp ? value.UdpPorts : "12";
        return (Mode == GameFilterMode.Udp ? udp : tcp, tcp, udp);
    }

    public static string ValidatePorts(string? input)
    {
        if (input == null || input.Length > 2048) throw new InvalidDataException("Слишком длинный или пустой список портов.");
        var ports = input.Replace(" ", "", StringComparison.Ordinal);
        if (!Regex.IsMatch(ports, @"\A[1-9][0-9]{0,4}(-[1-9][0-9]{0,4})?(,[1-9][0-9]{0,4}(-[1-9][0-9]{0,4})?)*\z", RegexOptions.CultureInvariant))
            throw new InvalidDataException("Порты: числа или диапазоны через запятую. Например: 1024-65535.");
        foreach (var item in ports.Split(','))
        {
            var pair = item.Split('-');
            var first = int.Parse(pair[0], CultureInfo.InvariantCulture);
            var last = int.Parse(pair[^1], CultureInfo.InvariantCulture);
            if (first > last || last > 65535) throw new InvalidDataException("Порты должны быть от 1 до 65535, начало диапазона не больше конца.");
        }
        return ports;
    }
}
