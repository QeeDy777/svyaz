using System.Xml.Linq;

namespace DiscordRoute.Core;

public static class LogonTask
{
    public static string Xml(string executable, string sid)
    {
        XNamespace ns = "http://schemas.microsoft.com/windows/2004/02/mit/task";
        XElement Node(string name, object? content) => new(ns + name, content);
        return new XDocument(new XDeclaration("1.0", "utf-16", null),
            new XElement(ns + "Task", new XAttribute("version", "1.4"),
                Node("RegistrationInfo", Node("Description", "DiscordRoute: автоматический подбор zapret после входа в Windows.")),
                Node("Triggers", Node("LogonTrigger", new[] { Node("Enabled", "true"), Node("UserId", sid), Node("Delay", "PT15S") })),
                Node("Principals", new XElement(ns + "Principal", new XAttribute("id", "User"),
                    Node("UserId", sid), Node("LogonType", "InteractiveToken"), Node("RunLevel", "HighestAvailable"))),
                Node("Settings", new[] { Node("MultipleInstancesPolicy", "IgnoreNew"), Node("DisallowStartIfOnBatteries", "false"),
                    Node("StopIfGoingOnBatteries", "false"), Node("StartWhenAvailable", "true"), Node("ExecutionTimeLimit", "PT0S"),
                    Node("Enabled", "true") }),
                new XElement(ns + "Actions", new XAttribute("Context", "User"),
                    Node("Exec", new[] { Node("Command", executable), Node("Arguments", "--startup") })))).ToString();
    }
}
