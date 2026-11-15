using System.Runtime.InteropServices;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal static class CurrentNetwork
{
    public static string? Read()
    {
        object? manager = null, networks = null;
        try
        {
            manager = Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("DCB00C01-570F-4A9B-8D69-199FDBA5723B"), true)!);
            networks = ((dynamic)manager!).GetNetworks(1);
            var ids = new List<Guid>();
            foreach (object network in (System.Collections.IEnumerable)networks)
            {
                try { ids.Add((Guid)((dynamic)network).GetNetworkId()); }
                finally { Marshal.ReleaseComObject(network); }
            }
            return NetworkProfiles.Identity(ids);
        }
        catch (Exception ex) when (ex is COMException or Microsoft.CSharp.RuntimeBinder.RuntimeBinderException or InvalidCastException)
        { return null; }
        finally
        {
            if (networks != null) Marshal.ReleaseComObject(networks);
            if (manager != null) Marshal.ReleaseComObject(manager);
        }
    }
}
