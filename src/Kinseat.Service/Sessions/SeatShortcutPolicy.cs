using System.Security.AccessControl;
using System.Security.Principal;
using MultiSeat.Service.Accounts;

namespace MultiSeat.Service.Sessions;

/// <summary>Only the host tools' public shortcuts are hidden from managed seat identities.</summary>
internal static class SeatShortcutPolicy
{
    private static readonly string[] Names = ["Kinseat Hôte.lnk", "HidHide Configurator.lnk"];

    internal static string[] SelectShortcuts(IEnumerable<string> paths) => paths
        .Where(path => Names.Contains(Path.GetFileName(path), StringComparer.OrdinalIgnoreCase))
        .Distinct(StringComparer.OrdinalIgnoreCase).ToArray();

    internal static void Apply(AccountManager accounts, ILogger logger)
    {
        if (!OperatingSystem.IsWindows()) return;
        var desktop = Environment.GetFolderPath(Environment.SpecialFolder.CommonDesktopDirectory);
        var paths = SelectShortcuts(Directory.EnumerateFiles(desktop, "*.lnk"));
        foreach (var account in accounts.ListManagedAccounts().Where(a => a.IsManaged))
        {
            if (!accounts.IsManagedSeatAccount(account.Username)) continue;
            var sid = AccountManager.TryResolveSid(account.Username, logger);
            if (sid is null) continue;
            foreach (var path in paths)
                try
                {
                    var file = new FileInfo(path);
                    var security = file.GetAccessControl();
                    var rule = new FileSystemAccessRule(new SecurityIdentifier(sid), FileSystemRights.Read,
                        AccessControlType.Deny);
                    var existing = security.GetAccessRules(true, false, typeof(SecurityIdentifier))
                        .OfType<FileSystemAccessRule>().Any(r => r.IdentityReference.Value == sid &&
                            r.AccessControlType == AccessControlType.Deny &&
                            (r.FileSystemRights & FileSystemRights.Read) == FileSystemRights.Read);
                    if (existing) continue;
                    security.AddAccessRule(rule);
                    file.SetAccessControl(security);
                    logger.LogInformation("Raccourci hôte masqué pour {Account} : {Shortcut}", account.Username, Path.GetFileName(path));
                }
                catch (Exception ex)
                {
                    logger.LogWarning(ex, "Masquage de {Shortcut} impossible pour {Account}", path, account.Username);
                }
        }
    }
}
