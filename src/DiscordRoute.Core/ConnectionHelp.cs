using System.Net.Sockets;

namespace DiscordRoute.Core;

public sealed class ConnectionFailure(string message, bool settings = false, bool requiresManualRetry = false) : IOException(message)
{
    public bool OpenSettings { get; } = settings;
    public bool RequiresManualRetry { get; } = requiresManualRetry;
}

public sealed record ConnectionHelp(string Message, bool OpenSettings = false)
{
    public string ActionLabel => OpenSettings ? "Открыть настройки" : "Повторить проверку";
    public static ConnectionHelp Describe(Exception error, bool networkAvailable, ProbeResult? probe = null)
    {
        if (error is ConnectionFailure known) return new(known.Message, known.OpenSettings);
        if (!networkAvailable) return new("Нет сетевого подключения. Подключитесь к Wi-Fi или кабельной сети; «Связь» повторит попытку.");
        if (error is UnauthorizedAccessException) return new("Нет доступа к файлам приложения. Перезапустите «Связь» с правами администратора.");
        if (error.GetBaseException() is SocketException { SocketErrorCode: SocketError.AddressAlreadyInUse or SocketError.AccessDenied })
            return new("Порт Telegram занят или недоступен. Остановите другой прокси или выберите свободный порт.", true);
        if (probe != null)
        {
            if (!probe.Api.Success && !probe.Api.Skipped) return new("Стратегии не открыли API Discord. Проверьте интернет и повторите подбор.");
            if (!probe.Gateway.Success && !probe.Gateway.Skipped) return new("Нет ответа Gateway Discord. Повторите подбор стратегии.");
            if (!probe.Voice.Success && !probe.Voice.Skipped) return new("Голосовой сервер Discord не ответил. Войдите в канал и повторите проверку; адрес можно изменить в настройках.", true);
            if (!probe.YouTube.Site.Skipped && !probe.YouTube.Success) return new("Стратегии не прошли проверку YouTube. Повторите подбор или проверьте интернет.");
        }
        return new("Не удалось установить соединение. Повторите проверку; технические подробности сохранены в журнале.");
    }
}
