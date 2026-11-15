using System.Security.Cryptography;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal static class RuntimeInstallation
{
    internal static readonly string[] Files = AppUpdatePackage.RuntimeFiles;

    public static string Install(string sourceExecutable, string installRoot)
    {
        if (!Path.GetFileName(sourceExecutable).Equals(Files[0], StringComparison.OrdinalIgnoreCase))
            throw new IOException("Автозапуск доступен в собранном Связь.exe.");
        var source = Path.GetDirectoryName(Path.GetFullPath(sourceExecutable))!;
        var identity = Identity(source);
        var versions = Path.Combine(installRoot, "Versions");
        var destination = Path.Combine(versions, identity);
        if (Directory.Exists(destination))
        {
            if (Identity(destination) != identity) throw new IOException("Установленная копия повреждена: " + destination);
            return Path.Combine(destination, Files[0]);
        }
        Directory.CreateDirectory(versions);
        var staging = Path.Combine(versions, ".staging-" + Guid.NewGuid().ToString("N"));
        try
        {
            Directory.CreateDirectory(staging);
            foreach (var name in Files) File.Copy(Path.Combine(source, name), Path.Combine(staging, name));
            foreach (var name in new[] { "DOTNET-LICENSE.txt", "DOTNET-THIRD-PARTY-NOTICES.txt", "WINDOWS-DESKTOP-LICENSE.txt", "TG-WS-PROXY-LICENSE.txt" })
                if (File.Exists(Path.Combine(source, name))) File.Copy(Path.Combine(source, name), Path.Combine(staging, name));
            if (Identity(staging) != identity) throw new IOException("Файлы приложения изменились при копировании.");
            Directory.Move(staging, destination);
        }
        finally { if (Directory.Exists(staging)) Directory.Delete(staging, true); }
        return Path.Combine(destination, Files[0]);
    }

    internal static string Identity(string directory)
    {
        using var combined = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        foreach (var name in Files)
        {
            using var stream = File.OpenRead(Path.Combine(directory, name));
            combined.AppendData(SHA256.HashData(stream));
        }
        return Convert.ToHexString(combined.GetHashAndReset()).ToLowerInvariant();
    }
}
