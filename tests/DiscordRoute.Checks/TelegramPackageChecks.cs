using System.Buffers.Binary;
using System.Security.Cryptography;
using System.Text.Json;
using DiscordRoute.Core;

internal static class TelegramPackageChecks
{
    public static async Task RunAsync(Action<bool, string> check, bool smoke)
    {
        var root = Path.Combine(Path.GetTempPath(), "svyaz-tg-packages-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(root);
        try
        {
            var bytes = new byte[256]; bytes[0] = (byte)'M'; bytes[1] = (byte)'Z';
            BinaryPrimitives.WriteInt32LittleEndian(bytes.AsSpan(60), 128);
            "PE\0\0"u8.CopyTo(bytes.AsSpan(128));
            BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(132), 0x8664);
            var digest = "sha256:" + Convert.ToHexStringLower(SHA256.HashData(bytes));
            var url = "https://github.com/Flowseal/tg-ws-proxy/releases/download/v1.11.4/TgWsProxy_windows.exe";
            string Json(string downloadUrl, string? hash, bool prerelease = false, string name = "TgWsProxy_windows.exe", long size = 256) =>
                JsonSerializer.Serialize(new { tag_name = "v1.11.4", draft = false, prerelease, assets = new[] { new { name, size, digest = hash, browser_download_url = downloadUrl } } });
            var release = TelegramPackageStore.ParseRelease(Json(url, digest));
            check(release.Version == "v1.11.4" && release.Size == 256, "Select only official stable Telegram Windows x64 asset");
            void Reject(Action action, string name)
            {
                try { action(); throw new Exception("Accepted invalid Telegram package: " + name); }
                catch (InvalidDataException) { check(true, name); }
            }
            foreach (var badUrl in new[] { url.Replace("github.com", "example.com"), url.Replace("https:", "http:"), url.Replace("Flowseal", "someone"), url + "?redirect=x", url.Replace("github.com/", "github.com:8443/") })
                Reject(() => TelegramPackageStore.ParseRelease(Json(badUrl, digest)), "Reject untrusted Telegram asset source");
            Reject(() => TelegramPackageStore.ParseRelease(Json(url, null)), "Reject Telegram asset without a digest");
            Reject(() => TelegramPackageStore.ParseRelease(Json(url, digest, prerelease: true)), "Reject Telegram prerelease");
            Reject(() => TelegramPackageStore.ParseRelease(Json(url, digest, name: "TgWsProxy_windows_arm64.exe")), "Reject Telegram ARM64 asset for x64 app");
            Reject(() => TelegramPackageStore.ParseRelease(Json(url, digest, size: 129 * 1024 * 1024)), "Reject oversized Telegram asset");
            var store = new TelegramPackageStore(root, _ => { });
            var active = store.Install(release, bytes);
            check(store.Open(active.Id) == active, "Telegram package metadata survives restart");
            TelegramPackageStore.Verify(active);
            Reject(() => store.Open("../../other"), "Reject Telegram package traversal");
            bytes[10]++;
            Reject(() => store.Install(release, bytes), "Reject modified Telegram download before installing");
            File.WriteAllBytes(Path.Combine(active.Root, TelegramPackageStore.Executable), bytes);
            Reject(() => TelegramPackageStore.Verify(active), "Reject Telegram binary tampering before launch");
            bytes[10]--;
            File.WriteAllBytes(Path.Combine(active.Root, TelegramPackageStore.Executable), bytes);
            bytes[132] = 0;
            Reject(() => store.Install(release with { Digest = "sha256:" + Convert.ToHexStringLower(SHA256.HashData(bytes)) }, bytes), "Reject non-x64 PE even with a matching digest");
            bytes[132] = 0x64;
            var pending = store.Install(release with { Version = "v-next" }, bytes);
            AppState State() => new() { Telegram = new TelegramSettings().WithSecret(), TelegramActivePackage = active.Id, TelegramPendingPackage = pending.Id };
            var state = State(); var runner = new FakeTelegramRunner();
            await new TelegramSession(runner, state, () => { }, _ => { }).StartAsync(active, pending, default);
            check(state.TelegramActivePackage == pending.Id && state.TelegramPendingPackage == null && runner.Current == pending, "Promote Telegram update only after successful startup");
            state = State(); runner = new() { FailId = pending.Id };
            var result = await new TelegramSession(runner, state, () => { }, _ => { }).StartAsync(active, pending, default);
            check(result == active && runner.Current == active && state.TelegramActivePackage == active.Id, "Failed Telegram update restores the previous component");
            state = State(); runner = new();
            await new TelegramSession(runner, state, () => throw new IOException("disk full"), _ => { }).StartAsync(active, pending, default);
            check(runner.Current == active && state.TelegramPendingPackage == pending.Id, "Telegram save failure restores previous process and settings");
            state = State(); runner = new(); using var cancel = new CancellationTokenSource();
            runner.BeforeStart = () => cancel.Cancel();
            try { await new TelegramSession(runner, state, () => { }, _ => { }).StartAsync(active, pending, cancel.Token); throw new Exception("Expected cancellation"); }
            catch (OperationCanceledException) { check(!runner.IsRunning && runner.Starts == 1 && state.TelegramActivePackage == active.Id, "Telegram cancellation stops the candidate without restarting the old process"); }
            if (smoke)
            {
                var actual = await store.StageLatestAsync(null, default) ?? throw new Exception("Expected official Telegram release");
                TelegramPackageStore.Verify(actual);
                check(new FileInfo(Path.Combine(actual.Root, TelegramPackageStore.Executable)).Length > 1_000_000, "Download official tg-ws-proxy and verify digest and x64 executable");
                check(await store.StageLatestAsync(actual.Version, default) == null, "Current Telegram version is not downloaded twice");
            }
        }
        finally { Directory.Delete(root, true); }
    }

    private sealed class FakeTelegramRunner : ITelegramRunner
    {
        public Package? Current { get; private set; }
        public string? FailId { get; init; }
        public Action? BeforeStart { get; set; }
        public int Starts { get; private set; }
        public bool IsRunning => Current != null;
        public Task StartAsync(Package package, TelegramSettings settings, CancellationToken ct)
        {
            Starts++; BeforeStart?.Invoke(); ct.ThrowIfCancellationRequested();
            if (package.Id == FailId) throw new IOException("test startup failure");
            Current = package; return Task.CompletedTask;
        }
        public Task StopAsync() { Current = null; return Task.CompletedTask; }
    }
}
