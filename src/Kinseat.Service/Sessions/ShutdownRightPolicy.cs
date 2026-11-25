using System.ComponentModel;
using System.Management;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Text.Json;
using MultiSeat.Service.Accounts;

namespace MultiSeat.Service.Sessions;

/// <summary>Host-wide LSA user rights: change only after all grants and local identities are known.</summary>
internal static class ShutdownRightPolicy
{
    private static readonly string[] Rights = ["SeShutdownPrivilege", "SeRemoteShutdownPrivilege"];
    private const string Admins = "S-1-5-32-544";
    // Any of these principals in a seat's token can grant the right indirectly.
    private static readonly HashSet<string> Shared = new(StringComparer.OrdinalIgnoreCase)
        { "S-1-1-0", "S-1-5-4", "S-1-5-11", "S-1-5-14", "S-1-5-32-545" };
    internal sealed record Change(string Right, string[] Remove, string[] Add);
    internal sealed record Snapshot(Change[] Changes);

    internal static Change[] Plan(IReadOnlyDictionary<string, string[]> original,
        IReadOnlySet<string> seats, IReadOnlySet<string> localUsers)
    {
        if (seats.Count == 0 || !seats.All(localUsers.Contains))
            throw new InvalidOperationException("Identité d'une place non confirmée comme utilisateur local");
        var changes = new List<Change>();
        foreach (var right in Rights)
        {
            if (!original.TryGetValue(right, out var grants)) throw new InvalidOperationException("Droit LSA non énuméré");
            var assigned = grants.ToHashSet(StringComparer.OrdinalIgnoreCase);
            // Unknown groups (including domain groups) may contain a seat: do not guess membership.
            if (assigned.Any(s => s != Admins && !Shared.Contains(s) && !localUsers.Contains(s)))
                throw new InvalidOperationException($"Attribution non identifiable pour {right}");
            if (assigned.Overlaps(seats))
                throw new InvalidOperationException($"Droit directement attribué à une place : {right}");
            var remove = assigned.Where(Shared.Contains).Order(StringComparer.OrdinalIgnoreCase).ToArray();
            var add = new List<string>();
            if (remove.Length != 0 && !assigned.Contains(Admins)) add.Add(Admins);
            if (right == Rights[0] && remove.Length != 0)
                add.AddRange(localUsers.Except(seats, StringComparer.OrdinalIgnoreCase)
                    .Where(s => !assigned.Contains(s)).Order(StringComparer.OrdinalIgnoreCase));
            changes.Add(new Change(right, remove, add.ToArray()));
        }
        return changes.ToArray();
    }

    internal static string[] RestoreTargets(IEnumerable<string> actual, IEnumerable<string> original) =>
        original.Except(actual, StringComparer.OrdinalIgnoreCase).ToArray();

    internal static void Apply(AccountManager accounts, ILogger logger)
    {
        if (!OperatingSystem.IsWindows()) return;
        var path = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
            "Kinseat", "shutdown-rights", "state.json");
        try
        {
            using var lsa = new LsaPolicy();
            if (File.Exists(path))
            {
                var snapshot = JsonSerializer.Deserialize<Snapshot>(File.ReadAllText(path))
                    ?? throw new InvalidDataException("Sauvegarde des droits LSA invalide");
                if (!ValidateSnapshot(snapshot)) throw new InvalidDataException("Sauvegarde des droits LSA invalide");
                if (!accounts.ListManagedAccounts().Any(a => a.IsManaged))
                {
                    // Reverse only our own changes, not an administrator's unrelated assignments.
                    foreach (var change in snapshot.Changes)
                    {
                        var actual = lsa.Enumerate(change.Right).ToHashSet(StringComparer.OrdinalIgnoreCase);
                        foreach (var sid in change.Remove.Where(s => !actual.Contains(s))) lsa.Add(change.Right, sid);
                        foreach (var sid in change.Add.Where(actual.Contains)) lsa.Remove(change.Right, sid);
                    }
                    File.Delete(path);
                    logger.LogInformation("Droits d'arrêt d'origine restaurés (plus aucune place gérée)");
                }
                else
                {
                    foreach (var right in Rights)
                        if (lsa.Enumerate(right).Any(Shared.Contains))
                            logger.LogWarning("Un groupe partagé détient de nouveau {Right} : protection des places non garantie", right);
                    logger.LogInformation("Droits d'arrêt déjà configurés ; sauvegarde conservée");
                }
                return;
            }
            var names = accounts.ListManagedAccounts().Where(a => a.IsManaged).Select(a => a.Username).ToArray();
            if (names.Length == 0) return;
            var seats = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach (var name in names)
            {
                if (!accounts.IsManagedSeatAccount(name)) throw new InvalidOperationException("Identité de place non vérifiable");
                var sid = new NTAccount(Environment.MachineName, name).Translate(typeof(SecurityIdentifier)).Value;
                seats.Add(sid);
            }
            var locals = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            using (var search = new ManagementObjectSearcher("SELECT PartOfDomain, UserName FROM Win32_ComputerSystem"))
            using (var result = search.Get())
            {
                using var computer = result.Cast<ManagementObject>().Single();
                if (computer["PartOfDomain"] is true)
                    throw new InvalidOperationException("Machine jointe à un domaine : comptes non locaux non préservables");
                // A local console account is included in the local SAM enumeration below.
                if (computer["UserName"] is string user && user.Contains('\\') &&
                    !user.StartsWith(Environment.MachineName + "\\", StringComparison.OrdinalIgnoreCase))
                    throw new InvalidOperationException("Compte console non local : retrait des droits refusé");
            }
            using (var search = new ManagementObjectSearcher("SELECT SID FROM Win32_UserAccount WHERE LocalAccount = TRUE"))
            using (var result = search.Get())
                foreach (ManagementObject row in result)
                {
                    using (row)
                        if (row["SID"] is string sid && sid.StartsWith("S-1-5-21-", StringComparison.Ordinal))
                            locals.Add(sid);
                }
            var originals = Rights.ToDictionary(r => r, r => lsa.Enumerate(r));
            var changes = Plan(originals, seats, locals);
            if (changes.All(c => c.Remove.Length == 0 && c.Add.Length == 0)) return;
            var store = new DirectoryInfo(Path.GetDirectoryName(path)!);
            store.Create();
            if ((store.Attributes & FileAttributes.ReparsePoint) != 0)
                throw new IOException("Répertoire de sauvegarde redirigé");
            var acl = store.GetAccessControl();
            acl.SetAccessRuleProtection(true, false);
            foreach (FileSystemAccessRule rule in acl.GetAccessRules(true, false, typeof(SecurityIdentifier)))
                acl.RemoveAccessRuleSpecific(rule);
            foreach (var principal in new[] { Admins, "S-1-5-18" })
                acl.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(principal),
                    FileSystemRights.FullControl, InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit,
                    PropagationFlags.None, AccessControlType.Allow));
            store.SetAccessControl(acl);
            // Save before the first LSA mutation: restart can recover even after a partial failure.
            var tmp = path + ".tmp";
            File.WriteAllText(tmp, JsonSerializer.Serialize(new Snapshot(changes)));
            File.Move(tmp, path, false);
            foreach (var change in changes)
            {
                // Grant console/non-seat local users BEFORE withdrawing shared grants.
                foreach (var sid in change.Add) lsa.Add(change.Right, sid);
                foreach (var sid in change.Remove) lsa.Remove(change.Right, sid);
            }
            logger.LogInformation("Droits d'arrêt des places retirés ; état d'origine conservé dans {Path}", path);
        }
        catch (Exception ex) { logger.LogWarning(ex, "Droits d'arrêt non modifiés ou partiellement appliqués ; vérifier/restaurer {Path}", path); }
    }

    private static bool ValidateSnapshot(Snapshot snapshot) => snapshot.Changes.Length == Rights.Length &&
        snapshot.Changes.Select(c => c.Right).Order().SequenceEqual(Rights.Order()) &&
        snapshot.Changes.All(c => c.Remove is not null && c.Add is not null &&
            c.Remove.All(Shared.Contains) && c.Add.All(s => s == Admins ||
                s.StartsWith("S-1-5-21-", StringComparison.Ordinal)));

    private sealed class LsaPolicy : IDisposable
    {
        private nint _handle;
        private const int PolicyAllAccess = 0x000F0FFF;
        private const int ObjectNameNotFound = unchecked((int)0xC0000034);
        private const int NoMoreEntries = unchecked((int)0x8000001A);
        [StructLayout(LayoutKind.Sequential)]
        private struct Attributes { public int Length; public nint Root; public nint Name; public int Flags; public nint Descriptor; public nint Qos; }
        [StructLayout(LayoutKind.Sequential)]
        private struct LsaString { public ushort Length; public ushort MaximumLength; public nint Buffer; }
        [DllImport("advapi32.dll")] private static extern int LsaOpenPolicy(nint system, ref Attributes attributes, int access, out nint policy);
        [DllImport("advapi32.dll")] private static extern int LsaEnumerateAccountsWithUserRight(nint policy, ref LsaString right, out nint buffer, out uint count);
        [DllImport("advapi32.dll")] private static extern int LsaAddAccountRights(nint policy, nint sid, [In] LsaString[] rights, uint count);
        [DllImport("advapi32.dll")] private static extern int LsaRemoveAccountRights(nint policy, nint sid, bool all, [In] LsaString[] rights, uint count);
        [DllImport("advapi32.dll")] private static extern int LsaClose(nint policy);
        [DllImport("advapi32.dll")] private static extern int LsaFreeMemory(nint buffer);
        [DllImport("advapi32.dll")] private static extern uint LsaNtStatusToWinError(int status);

        internal LsaPolicy()
        {
            var attrs = new Attributes { Length = Marshal.SizeOf<Attributes>() };
            Check(LsaOpenPolicy(0, ref attrs, PolicyAllAccess, out _handle));
        }
        private static void Check(int status)
        {
            if (status != 0) throw new Win32Exception((int)LsaNtStatusToWinError(status));
        }
        private static LsaString Name(string name, out nint buffer)
        {
            buffer = Marshal.StringToHGlobalUni(name);
            return new LsaString { Length = checked((ushort)(name.Length * 2)),
                MaximumLength = checked((ushort)((name.Length + 1) * 2)), Buffer = buffer };
        }
        internal string[] Enumerate(string right)
        {
            var name = Name(right, out var ptr);
            nint buffer = 0;
            try
            {
                var status = LsaEnumerateAccountsWithUserRight(_handle, ref name, out buffer, out var count);
                if (status == ObjectNameNotFound || status == NoMoreEntries) return [];
                Check(status);
                var values = new List<string>();
                for (var i = 0; i < count; i++)
                {
                    var sidPtr = Marshal.ReadIntPtr(buffer, checked((int)i * IntPtr.Size));
                    values.Add(new SecurityIdentifier(sidPtr).Value);
                }
                return values.ToArray();
            }
            finally { if (buffer != 0) LsaFreeMemory(buffer); Marshal.FreeHGlobal(ptr); }
        }
        private void Update(string right, string sid, bool add)
        {
            var name = Name(right, out var ptr);
            var bytes = new SecurityIdentifier(sid);
            var sidPtr = Marshal.AllocHGlobal(bytes.BinaryLength);
            try
            {
                var data = new byte[bytes.BinaryLength];
                bytes.GetBinaryForm(data, 0);
                Marshal.Copy(data, 0, sidPtr, data.Length);
                Check(add ? LsaAddAccountRights(_handle, sidPtr, [name], 1)
                    : LsaRemoveAccountRights(_handle, sidPtr, false, [name], 1));
            }
            finally { Marshal.FreeHGlobal(sidPtr); Marshal.FreeHGlobal(ptr); }
        }
        internal void Add(string right, string sid) => Update(right, sid, true);
        internal void Remove(string right, string sid) => Update(right, sid, false);
        public void Dispose() { if (_handle != 0) LsaClose(_handle); _handle = 0; }
    }
}
