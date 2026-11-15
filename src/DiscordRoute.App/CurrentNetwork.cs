using System.Runtime.InteropServices;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal static class CurrentNetwork
{
    // GetNetworkId returns a native GUID, which IDispatch/dynamic cannot marshal as a VARIANT.
    [ComImport, Guid("DCB00002-570F-4A9B-8D69-199FDBA5723B"), InterfaceType(ComInterfaceType.InterfaceIsDual)]
    private interface INetwork
    {
        [return: MarshalAs(UnmanagedType.BStr)] string GetName();
        void SetName([MarshalAs(UnmanagedType.BStr)] string name);
        [return: MarshalAs(UnmanagedType.BStr)] string GetDescription();
        void SetDescription([MarshalAs(UnmanagedType.BStr)] string description);
        Guid GetNetworkId();
    }

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
                try { ids.Add(((INetwork)network).GetNetworkId()); }
                finally { Marshal.ReleaseComObject(network); }
            }
            return NetworkProfiles.Identity(ids);
        }
        catch (Exception ex) when (ex is COMException or Microsoft.CSharp.RuntimeBinder.RuntimeBinderException or InvalidCastException or ArgumentException)
        { return null; }
        finally
        {
            if (networks != null) Marshal.ReleaseComObject(networks);
            if (manager != null) Marshal.ReleaseComObject(manager);
        }
    }
}
