using System.IO.Compression;
using System.Security.Cryptography;
using System.Text.Json;
using DiscordRoute.Core;

internal static class AppUpdateChecks
{
    public static void Run(Action<bool, string> check)
    {
        var root = Path.Combine(Path.GetTempPath(), "svyaz-update-checks-" + Guid.NewGuid().ToString("N"));
        var bytes = Zip(AppUpdatePackage.RuntimeFiles);
        var digest = "sha256:" + Convert.ToHexStringLower(SHA256.HashData(bytes));
        var name = "Svyaz-0.5.0-win-x64-unsigned.zip";
        var url = "https://github.com/QeeDy777/svyaz/releases/download/v0.5.0/" + name;
        string Metadata(string assetUrl, string hash = "", bool prerelease = false, string tag = "v0.5.0") => JsonSerializer.Serialize(new
        {
            tag_name = tag, draft = false, prerelease,
            assets = new[] { new { name, browser_download_url = assetUrl, digest = hash.Length == 0 ? digest : hash, size = bytes.Length } }
        });
        var release = AppUpdatePackage.ParseRelease(Metadata(url));
        check(release.Version == new Version(0, 5, 0) && release.Url.AbsoluteUri == url, "Self-update selects exact official x64 stable asset");
        foreach (var invalid in new[] { Metadata(url.Replace("QeeDy777", "attacker")), Metadata(url + "?redirect=evil"), Metadata(url.Replace("https:", "http:")), Metadata(url, "bad"), Metadata(url, prerelease: true), Metadata(url, tag: "v0.5.0-beta") })
            Reject(() => AppUpdatePackage.ParseRelease(invalid), "Reject invalid self-update metadata");
        foreach (var version in new[] { "01.2.3", "1.2", "1.2.3.4", "1.2.3-beta", "1.2.3\n" })
            Reject(() => AppUpdatePackage.ParseVersion(version), "Reject ambiguous app version");
        check(AppUpdatePackage.ParseVersion("0.10.0") > AppUpdatePackage.ParseVersion("0.9.0"), "Self-update compares numeric versions");
        try
        {
            AppUpdatePackage.Extract(release, bytes, root);
            check(AppUpdatePackage.RuntimeFiles.All(f => File.Exists(Path.Combine(root, f))), "Self-update extracts complete runtime into staging");
            Reject(() => AppUpdatePackage.Extract(release, bytes, root), "Self-update never overwrites existing files");
            var broken = bytes.ToArray(); broken[0] ^= 1;
            Reject(() => AppUpdatePackage.Extract(release, broken, root + "-bad"), "Reject damaged self-update ZIP");
            foreach (var names in new[] { AppUpdatePackage.RuntimeFiles[..5], AppUpdatePackage.RuntimeFiles.Append("../evil.exe").ToArray(), AppUpdatePackage.RuntimeFiles.Append("extra.dll").ToArray(), AppUpdatePackage.RuntimeFiles.Append("Связь.exe").ToArray() })
            {
                var invalid = Zip(names);
                var metadata = release with { Size = invalid.Length, Digest = "sha256:" + Convert.ToHexStringLower(SHA256.HashData(invalid)) };
                Reject(() => AppUpdatePackage.Extract(metadata, invalid, root + "-bad"), "Reject incomplete, escaping, unknown or duplicate update files");
                check(!Directory.Exists(root + "-bad"), "Unsafe update is rejected before extraction");
            }
            using var cancel = new CancellationTokenSource(); cancel.Cancel();
            try { AppUpdatePackage.Extract(release, bytes, root + "-cancel", cancel.Token); throw new Exception("Cancellation ignored"); }
            catch (OperationCanceledException) { check(true, "Self-update extraction respects cancellation"); }
        }
        finally
        {
            foreach (var directory in new[] { root, root + "-bad", root + "-cancel" })
                if (Directory.Exists(directory)) Directory.Delete(directory, true);
        }
        void Reject(Action action, string message)
        {
            try { action(); throw new Exception("Expected rejection: " + message); }
            catch (Exception ex) when (ex is IOException or InvalidDataException) { check(true, message); }
        }
    }

    private static byte[] Zip(IEnumerable<string> names)
    {
        using var output = new MemoryStream();
        using (var zip = new ZipArchive(output, ZipArchiveMode.Create, true))
            foreach (var name in names)
            { using var writer = new StreamWriter(zip.CreateEntry(name).Open()); writer.Write("fixture"); }
        return output.ToArray();
    }
}
