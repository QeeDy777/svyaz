namespace DiscordRoute.App;

internal sealed record ServiceIndicator(string Name, CheckKind Kind, string Detail)
{
    public string Label => Kind == CheckKind.Failed ? Name + " · ошибка" : Name;
    public string Summary => Label + " · " + Detail;
}
