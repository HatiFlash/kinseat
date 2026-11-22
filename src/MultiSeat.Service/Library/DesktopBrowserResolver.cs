using Microsoft.Win32;

namespace MultiSeat.Service.Library;

internal static class DesktopBrowserResolver
{
    internal static string? Choose(Func<string?> brave, Func<string?> association) =>
        Valid(brave()) ?? Valid(association());

    private static string? Valid(string? path) => !string.IsNullOrWhiteSpace(path) &&
        File.Exists(Environment.ExpandEnvironmentVariables(path)) ? Environment.ExpandEnvironmentVariables(path) : null;

    internal static string? Resolve(string account)
    {
        return Choose(() =>
        {
            using var app = Registry.LocalMachine.OpenSubKey(@"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\brave.exe");
            return app?.GetValue(null) as string;
        }, () =>
        {
            var sid = new System.Security.Principal.NTAccount(Environment.MachineName, account)
                .Translate(typeof(System.Security.Principal.SecurityIdentifier)).Value;
            using var choice = Registry.Users.OpenSubKey($@"{sid}\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\http\UserChoice");
            var progId = choice?.GetValue("ProgId") as string;
            if (!string.IsNullOrWhiteSpace(progId))
            {
                using var userClasses = Registry.Users.OpenSubKey($@"{sid}\Software\Classes\{progId}\shell\open\command");
                if (Executable(userClasses?.GetValue(null) as string) is { } userExe) return userExe;
            }
            if (!string.IsNullOrWhiteSpace(progId))
                foreach (var hive in new[] { Registry.LocalMachine })
                {
                    using var command = hive.OpenSubKey($@"Software\Classes\{progId}\shell\open\command");
                    if (Executable(command?.GetValue(null) as string) is { } exe) return exe;
                }
            using var fallback = Registry.ClassesRoot.OpenSubKey(@"http\shell\open\command");
            return Executable(fallback?.GetValue(null) as string);
        });
    }

    internal static string? Executable(string? command)
    {
        if (string.IsNullOrWhiteSpace(command)) return null;
        var text = Environment.ExpandEnvironmentVariables(command.Trim());
        if (text.StartsWith('"'))
        {
            var end = text.IndexOf('"', 1);
            return end > 1 ? text[1..end] : null;
        }
        var exe = text.IndexOf(".exe", StringComparison.OrdinalIgnoreCase);
        return exe < 0 ? null : text[..(exe + 4)];
    }
}
