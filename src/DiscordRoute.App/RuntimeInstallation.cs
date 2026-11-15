using System.Security.Cryptography;

namespace DiscordRoute.App;

internal static class RuntimeInstallation
{
    // Keep this list in sync with the self-contained WPF publish output.
    internal static readonly string[] Files = ["Связь.exe", "D3DCompiler_47_cor3.dll", "PenImc_cor3.dll",
        "PresentationNative_cor3.dll", "vcruntime140_cor3.dll", "wpfgfx_cor3.dll"];

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
            foreach (var name in new[] { "DOTNET-LICENSE.txt", "DOTNET-THIRD-PARTY-NOTICES.txt", "WINDOWS-DESKTOP-LICENSE.txt" })
                if (File.Exists(Path.Combine(source, name))) File.Copy(Path.Combine(source, name), Path.Combine(staging, name));
            if (Identity(staging) != identity) throw new IOException("Файлы приложения изменились при копировании.");
            Directory.Move(staging, destination);
        }
        finally { if (Directory.Exists(staging)) Directory.Delete(staging, true); }
        return Path.Combine(destination, Files[0]);
    }

    private static string Identity(string directory)
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
