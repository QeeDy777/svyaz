namespace DiscordRoute.Core;

public sealed record ServiceSelection
{
    public bool Discord { get; init; } = true;
    public bool YouTube { get; init; } = true;
    public bool Telegram { get; init; }
    public bool UsesZapret => Discord || YouTube;
    public ServiceSelection Validate() => Discord || YouTube || Telegram ? this
        : throw new InvalidDataException("Выберите хотя бы один сервис.");
}
