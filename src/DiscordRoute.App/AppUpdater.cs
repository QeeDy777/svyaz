using System.Diagnostics;
using System.Text.Json;
using System.Text.RegularExpressions;
using DiscordRoute.Core;

namespace DiscordRoute.App;

internal sealed record InstalledApp(string Version, string Id);

internal sealed class AppUpdater(string installRoot, string dataRoot)
{
    private string DirectoryPath => Path.Combine(dataRoot, "AppUpdate");
    internal static Version RunningVersion => AppUpdatePackage.ParseVersion(typeof(AppUpdater).Assembly.GetName().Version!.ToString(3));
    public AppUpdater() : this(Program.InstallRoot, Program.DataRoot) { }

    public InstalledApp? Read(string name)
    {
        var path = StatePath(name);
        if (!File.Exists(path)) return null;
        if (new FileInfo(path).Length > 4096) throw new InvalidDataException("Повреждены данные обновления «Связи».");
        var value = JsonSerializer.Deserialize<InstalledApp>(File.ReadAllText(path)) ?? throw new InvalidDataException("Пустые данные обновления.");
        Validate(value);
        return value;
    }

    public string Executable(InstalledApp value)
    {
        Validate(value);
        var root = Path.Combine(installRoot, "Versions", value.Id);
        if (RuntimeInstallation.Identity(root) != value.Id) throw new IOException("Установленная версия «Связи» повреждена.");
        var executable = Path.Combine(root, "Связь.exe");
        VerifyVersion(executable, value.Version);
        return executable;
    }

    public async Task<InstalledApp?> PrepareAsync(bool force, CancellationToken ct)
    {
        var pending = Read("pending");
        var current = RunningVersion;
        if (!force && pending != null && AppUpdatePackage.ParseVersion(pending.Version) > current) current = AppUpdatePackage.ParseVersion(pending.Version);
        var rejected = force ? null : Read("rejected");
        if (rejected != null && AppUpdatePackage.ParseVersion(rejected.Version) > current) current = AppUpdatePackage.ParseVersion(rejected.Version);
        var download = await AppUpdatePackage.DownloadAsync(current, ct);
        if (download == null) return pending != null && AppUpdatePackage.ParseVersion(pending.Version) > RunningVersion ? pending : null;
        return await Task.Run(() => Stage(download.Value.Release, download.Value.Bytes, ct), ct);
    }

    internal InstalledApp Stage(AppRelease release, byte[] bytes, CancellationToken ct)
    {
        var stage = Path.Combine(DirectoryPath, "stage-" + Guid.NewGuid().ToString("N"));
        try
        {
            AppUpdatePackage.Extract(release, bytes, stage, ct);
            var source = Path.Combine(stage, "Связь.exe");
            VerifyVersion(source, release.Version.ToString());
            ct.ThrowIfCancellationRequested();
            var installed = RuntimeInstallation.Install(source, installRoot);
            var result = new InstalledApp(release.Version.ToString(), Path.GetFileName(Path.GetDirectoryName(installed)!));
            Executable(result);
            ct.ThrowIfCancellationRequested();
            Write("pending", result);
            return result;
        }
        finally { if (Directory.Exists(stage)) Directory.Delete(stage, true); }
    }

    public InstalledApp? SelectForLaunch(bool applyPending)
    {
        var pending = applyPending ? Read("pending") : null;
        var current = Read("current");
        return new[] { pending, current }.Where(v => v != null && AppUpdatePackage.ParseVersion(v.Version) > RunningVersion)
            .OrderByDescending(v => AppUpdatePackage.ParseVersion(v!.Version)).FirstOrDefault();
    }

    public void Promote(InstalledApp value)
    {
        Executable(value);
        var pending = Read("pending");
        Write("current", value);
        if (pending == value)
            try { File.Delete(StatePath("pending")); } catch (IOException) { } catch (UnauthorizedAccessException) { }
    }

    public void Reject(InstalledApp value)
    {
        Write("rejected", value);
        if (Read("pending") == value) File.Delete(StatePath("pending"));
        if (Read("current") == value) File.Delete(StatePath("current"));
    }

    private string StatePath(string name) => Path.Combine(DirectoryPath, name switch
    { "pending" or "current" or "rejected" => name + ".json", _ => throw new ArgumentException("Unknown app update state") });

    private void Write(string name, InstalledApp value)
    {
        Validate(value);
        Directory.CreateDirectory(DirectoryPath);
        var path = StatePath(name);
        File.WriteAllText(path + ".tmp", JsonSerializer.Serialize(value));
        File.Move(path + ".tmp", path, true);
    }

    private static void Validate(InstalledApp value)
    {
        AppUpdatePackage.ParseVersion(value.Version);
        if (value.Id == null || !Regex.IsMatch(value.Id, @"\A[0-9a-f]{64}\z")) throw new InvalidDataException("Неверный каталог обновления.");
    }

    private static void VerifyVersion(string executable, string expected)
    {
        var info = FileVersionInfo.GetVersionInfo(executable);
        if (new Version(info.FileMajorPart, info.FileMinorPart, info.FileBuildPart) != AppUpdatePackage.ParseVersion(expected))
            throw new InvalidDataException("Версия EXE не совпадает с релизом.");
    }
}
