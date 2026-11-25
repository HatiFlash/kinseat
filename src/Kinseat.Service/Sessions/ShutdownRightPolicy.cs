using System.ComponentModel;
using System.Management;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Text.Json;
using Microsoft.Win32;
using MultiSeat.Service.Accounts;

namespace MultiSeat.Service.Sessions;

/// <summary>Host-wide LSA user rights via a dedicated local group for non-seat shutdown access.</summary>
internal static class ShutdownRightPolicy
{
    internal const string AllowedGroupName = "Kinseat-ArretAutorise";
    private const string AllowedGroupComment = "Comptes autorisés à arrêter le PC hôte (Kinseat)";
    private static readonly string[] Rights = ["SeShutdownPrivilege", "SeRemoteShutdownPrivilege"];
    internal const string AdminsSid = "S-1-5-32-544";
    internal const string BackupOperatorsSid = "S-1-5-32-551";
    private static readonly string[] ExcludedLocalRids = ["-501", "-503", "-504"];
    // Only broad well-known groups are ever removed; never Administrators, Backup Operators, or specific SIDs.
    private static readonly HashSet<string> BroadGroups = new(StringComparer.OrdinalIgnoreCase)
        { "S-1-1-0", "S-1-5-4", "S-1-5-11", "S-1-5-14", "S-1-5-32-545" };

    internal sealed record Change(string Right, string[] Remove, string[] Add);
    internal sealed record Snapshot(Change[] Changes, string? GroupName = null, string? GroupSid = null, bool Applied = false);
    internal sealed record HostAccounts(
        IReadOnlySet<string> ActiveLocalUsers,
        string? ConsoleUserSid,
        IReadOnlySet<string> NonLocalProfileSids,
        IReadOnlySet<string> PrivilegedMemberSids);

    internal interface ILsaPolicy : IDisposable
    {
        string[] Enumerate(string right);
        void Add(string right, string sid);
        void Remove(string right, string sid);
    }

    internal interface ILocalGroupPolicy
    {
        string EnsureGroup(string name, string comment);
        string[] GetMemberSids(string groupName);
        void AddMemberSid(string groupName, string sid);
        void RemoveMemberSid(string groupName, string sid);
        void DeleteGroup(string name);
    }

    internal static readonly string StatePath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
        "Kinseat", "shutdown-rights", "state.json");
    private static readonly object Gate = new();

    internal static Change[] Plan(IReadOnlyDictionary<string, string[]> original, string groupSid)
    {
        if (string.IsNullOrWhiteSpace(groupSid) || !groupSid.StartsWith("S-1-5-21-", StringComparison.OrdinalIgnoreCase))
            throw new InvalidOperationException("SID du groupe dédié invalide");
        var changes = new List<Change>();
        foreach (var right in Rights)
        {
            if (!original.TryGetValue(right, out var grants))
                throw new InvalidOperationException($"Droit LSA non énuméré : {right}");
            var assigned = grants.ToHashSet(StringComparer.OrdinalIgnoreCase);
            var remove = assigned.Where(BroadGroups.Contains).Order(StringComparer.OrdinalIgnoreCase).ToArray();
            var add = new List<string>();
            if (remove.Length != 0)
            {
                if (!assigned.Contains(groupSid)) add.Add(groupSid);
                if (!assigned.Contains(AdminsSid)) add.Add(AdminsSid);
            }
            changes.Add(new Change(right, remove, add.ToArray()));
        }
        return changes.ToArray();
    }

    internal static string[] RestoreTargets(IEnumerable<string> actual, IEnumerable<string> original) =>
        original.Except(actual, StringComparer.OrdinalIgnoreCase).ToArray();

    internal static bool IsNonLocalProfileSid(string sid, string? machineSidPrefix)
    {
        if (string.IsNullOrWhiteSpace(sid)) return false;
        if (sid.StartsWith("S-1-12-1-", StringComparison.OrdinalIgnoreCase)) return true;
        if (!sid.StartsWith("S-1-5-21-", StringComparison.OrdinalIgnoreCase)) return false;
        if (ExcludedLocalRids.Any(r => sid.EndsWith(r, StringComparison.OrdinalIgnoreCase))) return false;
        return string.IsNullOrEmpty(machineSidPrefix) ||
            !sid.StartsWith(machineSidPrefix, StringComparison.OrdinalIgnoreCase);
    }

    internal static string? ExtractMachineSidPrefix(string sid)
    {
        if (!sid.StartsWith("S-1-5-21-", StringComparison.OrdinalIgnoreCase)) return null;
        var lastDash = sid.LastIndexOf('-');
        return lastDash > 0 ? sid[..(lastDash + 1)] : null;
    }

    internal static void Apply(AccountManager accounts, ILogger logger)
    {
        if (!OperatingSystem.IsWindows()) return;
        lock (Gate)
        try
        {
            using var lsa = new LsaPolicy();
            var groups = new WindowsLocalGroupPolicy();
            var names = accounts.ListManagedAccounts().Where(a => a.IsManaged).Select(a => a.Username).ToArray();
            if (names.Length == 0)
            {
                Restore(lsa, groups, StatePath);
                return;
            }
            var seats = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach (var name in names)
            {
                if (!accounts.IsManagedSeatAccount(name))
                    throw new InvalidOperationException("Identité de place non vérifiable");
                seats.Add(new NTAccount(Environment.MachineName, name).Translate(typeof(SecurityIdentifier)).Value);
            }
            var groupSid = groups.EnsureGroup(AllowedGroupName, AllowedGroupComment);
            var hostAccounts = CollectHostAccounts(groups, ExtractMachineSidPrefix(groupSid), logger);
            ApplyCore(lsa, groups, StatePath, seats, hostAccounts, logger);
        }
        catch (Exception ex)
        {
            logger.LogWarning(ex, "Droits d'arrêt inchangés ou partiellement appliqués ; vérifier/restaurer {Path}", StatePath);
        }
    }

    internal static bool ApplyCore(
        ILsaPolicy lsa,
        ILocalGroupPolicy groups,
        string statePath,
        IReadOnlySet<string> managedSeats,
        HostAccounts hostAccounts,
        ILogger logger)
    {
        if (managedSeats.Count == 0)
        {
            Restore(lsa, groups, statePath);
            return true;
        }

        var privilegedSeats = managedSeats.Where(hostAccounts.PrivilegedMemberSids.Contains).ToArray();
        foreach (var seatSid in privilegedSeats)
        {
            logger.LogWarning(
                "Place {Sid} membre d'Administrators ou Backup Operators : non traitée comme protégée pour le droit d'arrêt",
                seatSid);
        }
        var protectedSeats = managedSeats.Except(privilegedSeats, StringComparer.OrdinalIgnoreCase)
            .ToHashSet(StringComparer.OrdinalIgnoreCase);
        if (protectedSeats.Count == 0)
            return false;

        var groupSid = groups.EnsureGroup(AllowedGroupName, AllowedGroupComment);
        var desiredMembers = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var sid in hostAccounts.ActiveLocalUsers) desiredMembers.Add(sid);
        if (!string.IsNullOrWhiteSpace(hostAccounts.ConsoleUserSid)) desiredMembers.Add(hostAccounts.ConsoleUserSid);
        foreach (var sid in hostAccounts.NonLocalProfileSids) desiredMembers.Add(sid);
        desiredMembers.ExceptWith(managedSeats);
        desiredMembers.Remove(groupSid);

        var currentMembers = groups.GetMemberSids(AllowedGroupName).ToHashSet(StringComparer.OrdinalIgnoreCase);
        foreach (var sid in desiredMembers.Where(s => !currentMembers.Contains(s)).Order(StringComparer.OrdinalIgnoreCase))
        {
            try
            {
                groups.AddMemberSid(AllowedGroupName, sid);
            }
            catch (Exception ex)
            {
                logger.LogWarning(ex,
                    "Impossible d'ajouter {Sid} au groupe {Group} : aucun droit d'arrêt retiré",
                    sid, AllowedGroupName);
                return false;
            }
        }
        foreach (var stale in currentMembers.Where(s => !desiredMembers.Contains(s)).Order(StringComparer.OrdinalIgnoreCase))
        {
            try { groups.RemoveMemberSid(AllowedGroupName, stale); }
            catch (Exception ex)
            {
                logger.LogWarning(ex, "Retrait du membre {Sid} hors du groupe {Group} impossible", stale, AllowedGroupName);
            }
        }

        if (File.Exists(statePath))
        {
            var existing = Load(statePath);
            if (existing.GroupSid is not null &&
                !string.Equals(existing.GroupSid, groupSid, StringComparison.OrdinalIgnoreCase))
            {
                logger.LogWarning("SID du groupe {Group} différent du journal {Path} ; restaurer avant de réappliquer",
                    AllowedGroupName, statePath);
                return false;
            }
            ApplySnapshot(lsa, statePath, existing);
            return true;
        }

        var originals = Rights.ToDictionary(r => r, r => lsa.Enumerate(r));
        foreach (var right in Rights)
        {
            foreach (var seatSid in protectedSeats.Where(s => originals[right].Contains(s, StringComparer.OrdinalIgnoreCase)))
            {
                logger.LogWarning("Droit {Right} directement attribué à la place {Sid} : attribution spécifique conservée", right, seatSid);
            }
        }

        var changes = Plan(originals, groupSid);
        if (changes.All(c => c.Remove.Length == 0 && c.Add.Length == 0)) return true;

        EnsureStateDirectory(statePath);
        // Persist original intent before the first LSA mutation; never overwrite an existing original.
        var snapshot = new Snapshot(changes, AllowedGroupName, groupSid, Applied: false);
        Save(statePath, snapshot);
        ApplySnapshot(lsa, statePath, Load(statePath));
        logger.LogInformation("Droits d'arrêt durcis via le groupe {Group} ; origine conservée dans {Path}", AllowedGroupName, statePath);
        return true;
    }

    internal static void ApplySnapshot(ILsaPolicy lsa, string path, Snapshot snapshot)
    {
        if (!ValidateSnapshot(snapshot)) throw new InvalidDataException("Sauvegarde des droits LSA invalide");
        foreach (var change in snapshot.Changes)
        {
            var actual = lsa.Enumerate(change.Right).ToHashSet(StringComparer.OrdinalIgnoreCase);
            // Always grant to the dedicated group (and Administrators) BEFORE removing broad groups.
            foreach (var sid in change.Add.Where(s => !actual.Contains(s))) lsa.Add(change.Right, sid);
            foreach (var sid in change.Remove.Where(actual.Contains)) lsa.Remove(change.Right, sid);
        }
        if (!snapshot.Applied) Save(path, snapshot with { Applied = true });
    }

    internal static void Restore(ILsaPolicy lsa, ILocalGroupPolicy? groups, string path)
    {
        if (!File.Exists(path)) return;
        var snapshot = Load(path);
        foreach (var change in snapshot.Changes)
        {
            var actual = lsa.Enumerate(change.Right).ToHashSet(StringComparer.OrdinalIgnoreCase);
            foreach (var sid in RestoreTargets(actual, change.Remove)) lsa.Add(change.Right, sid);
            foreach (var sid in change.Add.Where(s => !change.Remove.Contains(s, StringComparer.OrdinalIgnoreCase) &&
                actual.Contains(s))) lsa.Remove(change.Right, sid);
        }
        groups?.DeleteGroup(snapshot.GroupName ?? AllowedGroupName);
        File.Delete(path);
    }

    private static Snapshot Load(string path)
    {
        var snapshot = JsonSerializer.Deserialize<Snapshot>(File.ReadAllText(path));
        return snapshot is not null && ValidateSnapshot(snapshot) ? snapshot :
            throw new InvalidDataException("Sauvegarde des droits LSA invalide");
    }

    private static void Save(string path, Snapshot snapshot)
    {
        var temp = path + ".tmp";
        File.WriteAllText(temp, JsonSerializer.Serialize(snapshot));
        File.Move(temp, path, true);
    }

    private static void EnsureStateDirectory(string path)
    {
        if (OperatingSystem.IsWindows()) SecureDirectory(path);
        else Directory.CreateDirectory(Path.GetDirectoryName(path)!);
    }

    private static HostAccounts CollectHostAccounts(ILocalGroupPolicy groups, string? machineSidPrefix, ILogger logger)
    {
        var activeLocals = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        using (var search = new ManagementObjectSearcher("SELECT SID, Disabled FROM Win32_UserAccount WHERE LocalAccount = TRUE"))
        using (var result = search.Get())
        {
            foreach (ManagementObject row in result)
            {
                using (row)
                {
                    if (row["Disabled"] is true) continue;
                    if (row["SID"] is string sid && sid.StartsWith("S-1-5-21-", StringComparison.OrdinalIgnoreCase) &&
                        !ExcludedLocalRids.Any(r => sid.EndsWith(r, StringComparison.OrdinalIgnoreCase)))
                    {
                        activeLocals.Add(sid);
                    }
                }
            }
        }

        string? consoleSid = null;
        using (var search = new ManagementObjectSearcher("SELECT UserName FROM Win32_ComputerSystem"))
        using (var result = search.Get())
        {
            using var computer = result.Cast<ManagementObject>().FirstOrDefault();
            if (computer?["UserName"] is string user && !string.IsNullOrWhiteSpace(user))
            {
                try { consoleSid = new NTAccount(user).Translate(typeof(SecurityIdentifier)).Value; }
                catch (Exception ex) { logger.LogWarning(ex, "Résolution SID du compte console impossible : {User}", user); }
            }
        }

        var nonLocalProfiles = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        using (var profiles = Registry.LocalMachine.OpenSubKey(@"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList"))
        {
            if (profiles is not null)
            {
                foreach (var subKey in profiles.GetSubKeyNames())
                {
                    if (IsNonLocalProfileSid(subKey, machineSidPrefix))
                        nonLocalProfiles.Add(subKey);
                }
            }
        }

        var privileged = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var member in groups.GetMemberSids(ResolveWellKnownGroupName(WellKnownSidType.BuiltinAdministratorsSid, "Administrators")))
            privileged.Add(member);
        foreach (var member in groups.GetMemberSids(ResolveWellKnownGroupName(WellKnownSidType.BuiltinBackupOperatorsSid, "Backup Operators")))
            privileged.Add(member);

        return new HostAccounts(activeLocals, consoleSid, nonLocalProfiles, privileged);
    }

    private static string ResolveWellKnownGroupName(WellKnownSidType sidType, string fallback)
    {
        try
        {
            var full = new SecurityIdentifier(sidType, null).Translate(typeof(NTAccount)).Value;
            var sep = full.IndexOf('\\');
            return sep >= 0 ? full[(sep + 1)..] : full;
        }
        catch { return fallback; }
    }

    internal static void SecureDirectory(string path)
    {
        var store = new DirectoryInfo(Path.GetDirectoryName(path)!);
        store.Create();
        if ((store.Attributes & FileAttributes.ReparsePoint) != 0)
            throw new IOException("Répertoire de sauvegarde redirigé");
        var acl = store.GetAccessControl();
        acl.SetAccessRuleProtection(true, false);
        foreach (FileSystemAccessRule rule in acl.GetAccessRules(true, false, typeof(SecurityIdentifier)))
            acl.RemoveAccessRuleSpecific(rule);
        foreach (var principal in new[] { AdminsSid, "S-1-5-18" })
            acl.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(principal),
                FileSystemRights.FullControl, InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit,
                PropagationFlags.None, AccessControlType.Allow));
        store.SetAccessControl(acl);
    }

    internal static bool ValidateSnapshot(Snapshot snapshot) => snapshot.Changes is not null &&
        snapshot.Changes.Length == Rights.Length &&
        (snapshot.GroupName is null || string.Equals(snapshot.GroupName, AllowedGroupName, StringComparison.OrdinalIgnoreCase)) &&
        (snapshot.GroupSid is null || snapshot.GroupSid.StartsWith("S-1-5-21-", StringComparison.OrdinalIgnoreCase)) &&
        snapshot.Changes.All(c => c is not null && c.Right is not null) &&
        snapshot.Changes.Select(c => c.Right).Order().SequenceEqual(Rights.Order()) &&
        snapshot.Changes.All(c => c.Remove is not null && c.Add is not null &&
            c.Remove.All(BroadGroups.Contains) && c.Add.All(s => string.Equals(s, AdminsSid, StringComparison.OrdinalIgnoreCase) ||
                s.StartsWith("S-1-5-21-", StringComparison.OrdinalIgnoreCase)) &&
            !c.Remove.Intersect(c.Add, StringComparer.OrdinalIgnoreCase).Any());

    private sealed class WindowsLocalGroupPolicy : ILocalGroupPolicy
    {
        private const int NerrSuccess = 0;
        private const int ErrorMoreData = 234;
        private const int ErrorNoSuchAlias = 1376;
        private const int ErrorMemberNotInAlias = 1377;
        private const int ErrorMemberInAlias = 1378;
        private const int ErrorAliasExists = 1379;
        private const int NerrGroupNotFound = 2220;
        private const int NerrGroupExists = 2223;

        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        private struct LocalGroupInfo1 { public string Name; public string Comment; }

        [StructLayout(LayoutKind.Sequential)]
        private struct LocalGroupMembersInfo0 { public nint Sid; }

        [DllImport("netapi32.dll", CharSet = CharSet.Unicode)]
        private static extern int NetLocalGroupAdd(string? server, int level, ref LocalGroupInfo1 buf, out uint parmErr);
        [DllImport("netapi32.dll", CharSet = CharSet.Unicode)]
        private static extern int NetLocalGroupDel(string? server, string groupName);
        [DllImport("netapi32.dll", CharSet = CharSet.Unicode)]
        private static extern int NetLocalGroupGetMembers(string? server, string groupName, int level,
            out nint bufPtr, int prefMaxLen, out int entriesRead, out int totalEntries, ref nint resumeHandle);
        [DllImport("netapi32.dll", CharSet = CharSet.Unicode)]
        private static extern int NetLocalGroupAddMembers(string? server, string groupName, int level,
            ref LocalGroupMembersInfo0 buf, int totalEntries);
        [DllImport("netapi32.dll", CharSet = CharSet.Unicode)]
        private static extern int NetLocalGroupDelMembers(string? server, string groupName, int level,
            ref LocalGroupMembersInfo0 buf, int totalEntries);
        [DllImport("netapi32.dll")]
        private static extern int NetApiBufferFree(nint buffer);

        public string EnsureGroup(string name, string comment)
        {
            var info = new LocalGroupInfo1 { Name = name, Comment = comment };
            var status = NetLocalGroupAdd(null, 1, ref info, out _);
            if (status != NerrSuccess && status != ErrorAliasExists && status != NerrGroupExists)
                throw new Win32Exception(status);
            return new NTAccount(Environment.MachineName, name).Translate(typeof(SecurityIdentifier)).Value;
        }

        public string[] GetMemberSids(string groupName)
        {
            var members = new List<string>();
            nint resume = 0;
            int status;
            do
            {
                status = NetLocalGroupGetMembers(null, groupName, 0, out var buf, -1, out var read, out _, ref resume);
                if (status == NerrGroupNotFound || status == ErrorNoSuchAlias) return [];
                if (status != NerrSuccess && status != ErrorMoreData) throw new Win32Exception(status);
                try
                {
                    for (var i = 0; i < read; i++)
                    {
                        var sidPtr = Marshal.ReadIntPtr(buf, checked(i * IntPtr.Size));
                        if (sidPtr != 0) members.Add(new SecurityIdentifier(sidPtr).Value);
                    }
                }
                finally { if (buf != 0) NetApiBufferFree(buf); }
            } while (status == ErrorMoreData);
            return members.ToArray();
        }

        public void AddMemberSid(string groupName, string sid) => UpdateMemberSid(groupName, sid, add: true);
        public void RemoveMemberSid(string groupName, string sid) => UpdateMemberSid(groupName, sid, add: false);

        private static void UpdateMemberSid(string groupName, string sid, bool add)
        {
            var sidObj = new SecurityIdentifier(sid);
            var bytes = new byte[sidObj.BinaryLength];
            sidObj.GetBinaryForm(bytes, 0);
            var ptr = Marshal.AllocHGlobal(bytes.Length);
            try
            {
                Marshal.Copy(bytes, 0, ptr, bytes.Length);
                var info = new LocalGroupMembersInfo0 { Sid = ptr };
                var status = add
                    ? NetLocalGroupAddMembers(null, groupName, 0, ref info, 1)
                    : NetLocalGroupDelMembers(null, groupName, 0, ref info, 1);
                if (add && (status == NerrSuccess || status == ErrorMemberInAlias)) return;
                if (!add && (status == NerrSuccess || status == ErrorMemberNotInAlias || status == NerrGroupNotFound || status == ErrorNoSuchAlias)) return;
                throw new Win32Exception(status);
            }
            finally { Marshal.FreeHGlobal(ptr); }
        }

        public void DeleteGroup(string name)
        {
            var status = NetLocalGroupDel(null, name);
            if (status != NerrSuccess && status != NerrGroupNotFound && status != ErrorNoSuchAlias)
                throw new Win32Exception(status);
        }
    }

    private sealed class LsaPolicy : ILsaPolicy
    {
        private nint _handle;
        private const int PolicyAccess = 0x00000810;
        private const int ObjectNameNotFound = unchecked((int)0xC0000034);
        private const int NoMoreEntries = unchecked((int)0x8000001A);
        [StructLayout(LayoutKind.Sequential)]
        private struct Attributes { public int Length; public nint Root; public nint Name; public int Flags; public nint Descriptor; public nint Qos; }
        [StructLayout(LayoutKind.Sequential)]
        private struct LsaString { public ushort Length; public ushort MaximumLength; public nint Buffer; }
        [DllImport("advapi32.dll")] private static extern int LsaOpenPolicy(nint system, ref Attributes attributes, int access, out nint policy);
        [DllImport("advapi32.dll")] private static extern int LsaEnumerateAccountsWithUserRight(nint policy, ref LsaString right, out nint buffer, out uint count);
        [DllImport("advapi32.dll")] private static extern int LsaAddAccountRights(nint policy, nint sid, [In] LsaString[] rights, uint count);
        [DllImport("advapi32.dll")] private static extern int LsaRemoveAccountRights(nint policy, nint sid, [MarshalAs(UnmanagedType.U1)] bool all, [In] LsaString[] rights, uint count);
        [DllImport("advapi32.dll")] private static extern int LsaClose(nint policy);
        [DllImport("advapi32.dll")] private static extern int LsaFreeMemory(nint buffer);
        [DllImport("advapi32.dll")] private static extern uint LsaNtStatusToWinError(int status);

        internal LsaPolicy()
        {
            var attrs = new Attributes { Length = Marshal.SizeOf<Attributes>() };
            Check(LsaOpenPolicy(0, ref attrs, PolicyAccess, out _handle));
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
        public string[] Enumerate(string right)
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
                var status = add ? LsaAddAccountRights(_handle, sidPtr, [name], 1)
                    : LsaRemoveAccountRights(_handle, sidPtr, false, [name], 1);
                if (!add && status == ObjectNameNotFound) return;
                Check(status);
            }
            finally { Marshal.FreeHGlobal(sidPtr); Marshal.FreeHGlobal(ptr); }
        }
        public void Add(string right, string sid) => Update(right, sid, true);
        public void Remove(string right, string sid) => Update(right, sid, false);
        public void Dispose() { if (_handle != 0) LsaClose(_handle); _handle = 0; }
    }
}
