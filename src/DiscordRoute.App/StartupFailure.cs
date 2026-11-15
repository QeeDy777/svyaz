namespace DiscordRoute.App;

internal static class StartupFailure
{
    public static string? Save(Exception error, string directory)
    {
        try
        {
            Directory.CreateDirectory(directory);
            var path = Path.Combine(directory, "startup-error.log");
            File.WriteAllText(path, DateTimeOffset.UtcNow.ToString("O") + Environment.NewLine + error);
            return path;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return null;
        }
    }
}
