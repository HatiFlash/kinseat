using System.Security.AccessControl;
using System.Security.Principal;
using System.Text.Json;

namespace MultiSeat.Service.Sessions;

/// <summary>Owns only the exact explicit deny it recorded on the public desktop.</summary>
public sealed class PublicDesktopFilter
{
    private const FileSystemRights Rights = FileSystemRights.ListDirectory | FileSystemRights.ReadData |
        FileSystemRights.ReadAttributes | FileSystemRights.ReadExtendedAttributes | FileSystemRights.ReadPermissions;
    private readonly string _desktop;
    private readonly string _inventory;
    private readonly object _gate = new();
    internal sealed record Entry(string Sid, string Account, DateTimeOffset Date);

    public PublicDesktopFilter(string? desktop = null, string? inventory = null)
    {
        _desktop = desktop ?? Environment.GetFolderPath(Environment.SpecialFolder.CommonDesktopDirectory);
        _inventory = inventory ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
            "MultiSeat", "public-desktop-denies.json");
    }

    private List<Entry> Load() => File.Exists(_inventory)
        ? JsonSerializer.Deserialize<List<Entry>>(File.ReadAllText(_inventory)) ?? throw new JsonException("Inventaire vide")
        : [];

    private void Save(List<Entry> entries)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(_inventory)!);
        var temp = _inventory + "." + Guid.NewGuid().ToString("N") + ".tmp";
        try { File.WriteAllText(temp, JsonSerializer.Serialize(entries)); File.Move(temp, _inventory, true); }
        finally { if (File.Exists(temp)) File.Delete(temp); }
    }

    private static FileSystemAccessRule Rule(SecurityIdentifier sid) =>
        new(sid, Rights, InheritanceFlags.None, PropagationFlags.None, AccessControlType.Deny);

    private static bool Matches(AuthorizationRule rule, SecurityIdentifier sid) =>
        rule is FileSystemAccessRule ace && ace.IdentityReference == sid && !ace.IsInherited &&
        ace.AccessControlType == AccessControlType.Deny && ace.FileSystemRights == Rights &&
        ace.InheritanceFlags == InheritanceFlags.None && ace.PropagationFlags == PropagationFlags.None;

    /// <param name="eligible">Verified managed, non-admin, non-console account; fail closed.</param>
    public void Apply(string sidValue, string account, bool enabled, bool eligible)
    {
        lock (_gate)
        {
            var entries = Load();
            var owned = entries.FirstOrDefault(e => e.Sid == sidValue &&
                e.Account.Equals(account, StringComparison.OrdinalIgnoreCase));
            if (!eligible && enabled) return;
            if (owned is null && !enabled) return;
            var sid = new SecurityIdentifier(sidValue);
            var directory = new DirectoryInfo(_desktop);
            var acl = directory.GetAccessControl();
            var present = acl.GetAccessRules(true, false, typeof(SecurityIdentifier)).Cast<AuthorizationRule>()
                .Any(rule => Matches(rule, sid));
            if (enabled)
            {
                if (owned is not null) return;
                // A foreign identical deny is not ours; do not claim it in the inventory.
                if (present) return;
                acl.AddAccessRule(Rule(sid));
                directory.SetAccessControl(acl);
                entries.Add(new Entry(sidValue, account, DateTimeOffset.UtcNow));
                try { Save(entries); }
                catch
                {
                    var rollback = directory.GetAccessControl();
                    rollback.RemoveAccessRuleSpecific(Rule(sid));
                    directory.SetAccessControl(rollback);
                    throw;
                }
            }
            else
            {
                if (present)
                {
                    acl.RemoveAccessRuleSpecific(Rule(sid));
                    directory.SetAccessControl(acl);
                }
                entries.Remove(owned!);
                Save(entries);
            }
        }
    }
}
