using System.IO.Compression;
using System.Security.Cryptography;

namespace DiscordRoute.App;

internal static class TelegramRuntime
{
    internal const string Version = "1.11.4";
    private static readonly object Gate = new();

    internal static string Prepare(string cacheRoot)
    {
        lock (Gate)
        {
            using var resource = typeof(TelegramRuntime).Assembly.GetManifestResourceStream("TelegramRuntime.zip")
                ?? throw new IOException("В сборке отсутствует встроенный движок Telegram.");
            var digest = Convert.ToHexStringLower(SHA256.HashData(resource));
            resource.Position = 0;
            var root = Path.Combine(cacheRoot, digest);
            using var archive = new ZipArchive(resource);
            for (var directory = new DirectoryInfo(cacheRoot); directory != null; directory = directory.Parent)
                if (directory.Exists && (directory.Attributes & FileAttributes.ReparsePoint) != 0)
                    throw new IOException("Каталог движка Telegram не должен быть ссылкой.");
            if (!Directory.Exists(root))
            {
                Directory.CreateDirectory(cacheRoot);
                var staging = Path.Combine(cacheRoot, Guid.NewGuid().ToString("N"));
                try
                {
                    archive.ExtractToDirectory(staging);
                    Directory.Move(staging, root);
                }
                finally { if (Directory.Exists(staging)) Directory.Delete(staging, true); }
            }
            if ((File.GetAttributes(cacheRoot) & FileAttributes.ReparsePoint) != 0 ||
                (File.GetAttributes(root) & FileAttributes.ReparsePoint) != 0)
                throw new IOException("Каталог движка Telegram не должен быть ссылкой.");
            var directories = new Stack<string>();
            directories.Push(root);
            var fileCount = 0;
            while (directories.TryPop(out var current))
                foreach (var path in Directory.EnumerateFileSystemEntries(current))
                {
                    var attributes = File.GetAttributes(path);
                    if ((attributes & FileAttributes.ReparsePoint) != 0)
                        throw new IOException("Каталог движка Telegram изменён.");
                    if ((attributes & FileAttributes.Directory) != 0) directories.Push(path);
                    else fileCount++;
                }
            if (fileCount != archive.Entries.Count)
                throw new IOException("Набор файлов движка Telegram изменён.");
            foreach (var entry in archive.Entries)
            {
                var path = Path.GetFullPath(Path.Combine(root, entry.FullName));
                if (!path.StartsWith(root + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase) ||
                    !File.Exists(path) || (File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0 || new FileInfo(path).Length != entry.Length)
                    throw new IOException("Файлы движка Telegram повреждены.");
                using var actual = File.OpenRead(path);
                using var expected = entry.Open();
                if (!CryptographicOperations.FixedTimeEquals(SHA256.HashData(actual), SHA256.HashData(expected)))
                    throw new IOException("Контрольная сумма движка Telegram не совпадает.");
            }
            return root;
        }
    }
}
