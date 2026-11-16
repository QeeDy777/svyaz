namespace DiscordRoute.Core;

public sealed record ResponseQuality(int Successful, int Total, long? MedianMs, long? SpreadMs, int Measured)
{
    public double Score => (double)(MedianMs ?? long.MaxValue / 2) + (SpreadMs ?? 0);
    public bool CanRank => Total >= 3 && Successful == Total && Measured == Total;
    public string Summary => Total == 0 ? "Замеров пока нет" : MedianMs == null ? $"Успешно {Successful}/{Total} · время не измерено" :
        $"Отклик {MedianMs} мс · " + (SpreadMs is { } spread ? $"разброс {spread} мс" : "нужен ещё замер") + $" · успешно {Successful}/{Total}";

    public static ResponseQuality From(IEnumerable<CheckResult> source)
    {
        var samples = source.Where(s => !s.Skipped).ToArray();
        var times = samples.Where(s => s.Success && s.ElapsedMs is >= 0).Select(s => s.ElapsedMs!.Value).Order().ToArray();
        long? median = times.Length == 0 ? null : times[(times.Length - 1) / 2] + (times[times.Length / 2] - times[(times.Length - 1) / 2]) / 2;
        return new(samples.Count(s => s.Success), samples.Length, median, times.Length < 2 ? null : times[^1] - times[0], times.Length);
    }

    public static CheckResult Sample(ProbeResult result)
    {
        var checks = new[] { result.Api, result.Gateway, result.Voice, result.YouTube.Site, result.YouTube.GoogleVideo }.Where(c => !c.Skipped).ToArray();
        return new(result.Success, "Отклик выбранных сервисов", ElapsedMs: result.Success && checks.All(c => c.ElapsedMs is >= 0)
            ? checks.Max(c => c.ElapsedMs) : null);
    }
}

public sealed class ResponseHistory
{
    private readonly Queue<CheckResult> samples = new();
    public ResponseQuality Quality => ResponseQuality.From(samples);
    public void Add(CheckResult sample)
    {
        if (sample.Skipped) return;
        samples.Enqueue(sample);
        while (samples.Count > 5) samples.Dequeue();
    }
    public void Clear() => samples.Clear();
}
