using System.Security.Cryptography;
using System.Text;

namespace DiscordRoute.Core;

public static class NetworkProfiles
{
    public static string? Identity(IEnumerable<Guid> networks)
    {
        var ids = networks.Where(id => id != Guid.Empty).Distinct().Order().ToArray();
        return ids.Length == 0 ? null : Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(string.Join(";", ids))));
    }

    public static bool Remember(ConvenienceState state, string? startedNetwork, string? currentNetwork, string strategy)
    {
        if (startedNetwork == null || startedNetwork != currentNetwork) return false;
        state.NetworkStrategies ??= new();
        if (!state.NetworkStrategies.ContainsKey(startedNetwork) && state.NetworkStrategies.Count >= 32)
            state.NetworkStrategies.Remove(state.NetworkStrategies.Keys.First());
        state.NetworkStrategies[startedNetwork] = strategy;
        return true;
    }
}
