using System.Security.AccessControl;
using System.Security.Principal;
using System.Text.Json;
using MultiSeat.Service.Accounts;

namespace MultiSeat.Service.Sessions;

/// <summary>Only the host tools' public shortcuts are hidden from managed seat identities.</summary>
internal static class SeatShortcutPolicy
{
    private static readonly string[] Names = ["Kinseat Hôte.lnk", "HidHide Configurator.lnk"];
    internal sealed record ShortcutEntry(string Path, string Sid);
    internal static readonly string InventoryPath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
        "Kinseat", "shutdown-rights", "shortcuts.json");
    private static readonly object Gate = new();

    private static FileSystemAccessRule Rule(string sid) =>
        new(new SecurityIdentifier(sid), FileSystemRights.Read, AccessControlType.Deny);

    private static bool OwnRule(FileSystemAccessRule r, string sid) =>
        !r.IsInherited && string.Equals(r.IdentityReference.Value, sid, StringComparison.OrdinalIgnoreCase) &&
        r.AccessControlType == AccessControlType.Deny && r.FileSystemRights == FileSystemRights.Read;

    private static List<ShortcutEntry> Load() => File.Exists(InventoryPath)
        ? JsonSerializer.Deserialize<List<ShortcutEntry>>(File.ReadAllText(InventoryPath))
            ?? throw new InvalidDataException("Inventaire des raccourcis invalide")
        : [];

    private static void Save(List<ShortcutEntry> entries)
    {
        ShutdownRightPolicy.SecureDirectory(InventoryPath);
        var tmp = InventoryPath + ".tmp";
        File.WriteAllText(tmp, JsonSerializer.Serialize(entries));
        File.Move(tmp, InventoryPath, true);
    }

    internal static string[] SelectShortcuts(IEnumerable<string> paths) => paths
        .Where(path => Names.Contains(Path.GetFileName(path), StringComparer.OrdinalIgnoreCase))
        .Distinct(StringComparer.OrdinalIgnoreCase).ToArray();

    internal static void Apply(AccountManager accounts, ILogger logger)
    {
        if (!OperatingSystem.IsWindows()) return;
        lock (Gate)
        {
            var desktop = Environment.GetFolderPath(Environment.SpecialFolder.CommonDesktopDirectory);
            var programs = Environment.GetFolderPath(Environment.SpecialFolder.CommonPrograms);
            var paths = SelectShortcuts(new[] { desktop, programs }
                .Where(Directory.Exists).SelectMany(dir => Directory.EnumerateFiles(dir, "*.lnk")));
            var eligible = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach (var account in accounts.ListManagedAccounts().Where(a => a.IsManaged))
            {
                if (!accounts.IsManagedSeatAccount(account.Username)) continue;
                var sid = AccountManager.TryResolveSid(account.Username, logger);
                if (sid is not null) eligible.Add(sid);
            }
            var entries = Load();
            foreach (var old in entries.ToArray())
            {
                if (eligible.Contains(old.Sid) && paths.Contains(old.Path, StringComparer.OrdinalIgnoreCase)) continue;
                try
                {
                    if (File.Exists(old.Path))
                    {
                        var file = new FileInfo(old.Path);
                        var acl = file.GetAccessControl();
                        acl.RemoveAccessRuleSpecific(Rule(old.Sid));
                        file.SetAccessControl(acl);
                    }
                    entries.Remove(old);
                    Save(entries);
                }
                catch (Exception ex) { logger.LogWarning(ex, "Nettoyage ACL du raccourci impossible : {Path}", old.Path); }
            }
            foreach (var sid in eligible)
            foreach (var path in paths)
            {
                var owned = entries.Any(e => e.Sid.Equals(sid, StringComparison.OrdinalIgnoreCase) &&
                    e.Path.Equals(path, StringComparison.OrdinalIgnoreCase));
                try
                {
                    var file = new FileInfo(path);
                    var acl = file.GetAccessControl();
                    if (acl.GetAccessRules(true, false, typeof(SecurityIdentifier))
                        .OfType<FileSystemAccessRule>().Any(r => OwnRule(r, sid))) continue;
                    if (!owned)
                    {
                        entries.Add(new ShortcutEntry(path, sid));
                        Save(entries); // Write intent before the ACL; recovery and uninstall can undo it.
                    }
                    acl.AddAccessRule(Rule(sid));
                    file.SetAccessControl(acl);
                }
                catch (Exception ex) { logger.LogWarning(ex, "Masquage de {Shortcut} impossible pour {Sid}", path, sid); }
            }
        }
    }
}
