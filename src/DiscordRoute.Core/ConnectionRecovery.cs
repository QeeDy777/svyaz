namespace DiscordRoute.Core;

// Intent is separate from process state: a manual stop must never reconnect later.
public sealed class ConnectionRecovery
{
    public bool Wanted { get; private set; }
    public bool Pending { get; private set; }
    public int Attempts { get; private set; }
    public DateTimeOffset Due { get; private set; }
    private DateTimeOffset? failedSince;
    private bool announced;
    public void Enable() { Stop(); Wanted = true; }
    public void Stop() { Wanted = Pending = announced = false; Attempts = 0; failedSince = null; }
    public void Invalidate(DateTimeOffset now)
    {
        if (!Wanted) return;
        Pending = true;
        Attempts = 0;
        failedSince ??= now;
        Due = now.AddSeconds(10);
    }
    public bool TryBegin(DateTimeOffset now, bool networkAvailable)
    {
        if (!Wanted || !Pending || !networkAvailable || Attempts >= 3 || now < Due) return false;
        Attempts++;
        Due = now.AddSeconds(Attempts * 30);
        return true;
    }
    public void Failed(DateTimeOffset now)
    {
        if (!Wanted) return;
        failedSince ??= now;
        if (!Pending) Due = now.AddSeconds(10);
        Pending = true;
    }
    public void Succeeded() { Pending = false; Attempts = 0; failedSince = null; announced = false; }
    public bool ShouldNotify(DateTimeOffset now, bool enabled)
    {
        if (!enabled || !Wanted || announced || failedSince == null || now - failedSince < TimeSpan.FromMinutes(2)) return false;
        announced = true;
        return true;
    }
}
