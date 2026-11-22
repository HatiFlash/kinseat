using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Text.Json;
using MultiSeat.Service.Ipc;

namespace MultiSeat.Service.Library;

/// <summary>Effective file access, including account groups (not a textual inspection of ACEs).</summary>
internal static class LibraryAccess
{
    private const int ReadExecute = 0x1200A9; // FILE_GENERIC_READ | FILE_GENERIC_EXECUTE

    [StructLayout(LayoutKind.Sequential)]
    private struct Request { public int DesiredAccess; public IntPtr PrincipalSelfSid; public IntPtr ObjectTypeList; public int ObjectTypeListLength; public IntPtr OptionalArguments; }
    [StructLayout(LayoutKind.Sequential)]
    private struct Response { public int ResultListLength; public IntPtr GrantedAccessMask; public IntPtr SaclEvaluationResults; public IntPtr Error; }

    [DllImport("authz.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool AuthzInitializeResourceManager(int flags, IntPtr access, IntPtr compute, IntPtr free,
        string name, out IntPtr manager);
    [DllImport("authz.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool AuthzInitializeContextFromSid(int flags, byte[] sid, IntPtr manager,
        IntPtr expiration, Luid id, IntPtr dynamicGroups, out IntPtr context);
    [StructLayout(LayoutKind.Sequential)] private struct Luid { public uint Low; public int High; }
    [DllImport("authz.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool AuthzAccessCheck(int flags, IntPtr context, ref Request request, IntPtr audit,
        byte[] securityDescriptor, IntPtr optionalDescriptors, int optionalCount, ref Response reply, IntPtr results);
    [DllImport("authz.dll")] private static extern bool AuthzFreeContext(IntPtr context);
    [DllImport("authz.dll")] private static extern bool AuthzFreeResourceManager(IntPtr manager);

    internal static string Status(GameEntry game, string account)
    {
        if (string.IsNullOrWhiteSpace(game.ExePath)) return string.IsNullOrWhiteSpace(game.LaunchUri) ? "unknown" : "ok";
        if (!File.Exists(game.ExePath)) return "missing";
        if (!OperatingSystem.IsWindows()) return "unknown";
        IntPtr manager = IntPtr.Zero, context = IntPtr.Zero, granted = IntPtr.Zero, error = IntPtr.Zero;
        try
        {
            var sid = (SecurityIdentifier)new NTAccount(Environment.MachineName, account).Translate(typeof(SecurityIdentifier));
            var bytes = new byte[sid.BinaryLength];
            sid.GetBinaryForm(bytes, 0);
            if (!AuthzInitializeResourceManager(0, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero, "Kinseat library", out manager) ||
                !AuthzInitializeContextFromSid(0, bytes, manager, IntPtr.Zero, default, IntPtr.Zero, out context))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            var descriptor = new FileInfo(game.ExePath).GetAccessControl().GetSecurityDescriptorBinaryForm();
            granted = Marshal.AllocHGlobal(sizeof(int)); error = Marshal.AllocHGlobal(sizeof(int));
            var request = new Request { DesiredAccess = ReadExecute };
            var response = new Response { ResultListLength = 1, GrantedAccessMask = granted, Error = error };
            if (!AuthzAccessCheck(0, context, ref request, IntPtr.Zero, descriptor, IntPtr.Zero, 0, ref response, IntPtr.Zero))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            var result = Marshal.ReadInt32(error);
            if (result == 5) return "denied";
            if (result != 0) return "unknown";
            return (Marshal.ReadInt32(granted) & ReadExecute) == ReadExecute ? "ok" : "denied";
        }
        catch (Exception) { return "unknown"; }
        finally
        {
            if (granted != IntPtr.Zero) Marshal.FreeHGlobal(granted);
            if (error != IntPtr.Zero) Marshal.FreeHGlobal(error);
            if (context != IntPtr.Zero) AuthzFreeContext(context);
            if (manager != IntPtr.Zero) AuthzFreeResourceManager(manager);
        }
    }

    internal static bool AllowedDirectory(string path)
    {
        if (string.IsNullOrWhiteSpace(path)) return false;
        var p = path.Replace('/', '\\');
        if (p.StartsWith(@"\\?\", StringComparison.Ordinal)) p = p[4..];
        p = p.TrimEnd('\\');
        if (!System.Text.RegularExpressions.Regex.IsMatch(p, @"^[A-Za-z]:\\") ||
            p.Split('\\').Any(s => s is "." or ".." || s.Length == 0)) return false;
        var parts = p.Split('\\');
        if (parts.Length < 2) return false;
        if (parts[1].Equals("Windows", StringComparison.OrdinalIgnoreCase)) return false;
        if (parts.Length == 2 && new[] { "Users", "Program Files", "Program Files (x86)", "ProgramData" }
                .Contains(parts[1], StringComparer.OrdinalIgnoreCase)) return false;
        if (parts.Length == 3 && parts[1].Equals("Users", StringComparison.OrdinalIgnoreCase)) return false;
        if (parts.Length == 4 && parts[1].Equals("Users", StringComparison.OrdinalIgnoreCase)) return false;
        return true;
    }
}

/// <summary>Records only ACEs explicitly added by Kinseat, and removes only those rules.</summary>
internal sealed class LibraryGrants
{
    private sealed record Grant(string GameId, string Account, string Directory, string Sid, bool OwnsRule = true);
    private readonly string _path;
    private readonly object _gate = new();
    private readonly JsonSerializerOptions _json = new(JsonSerializerDefaults.Web);
    internal LibraryGrants(string? path = null) => _path = path ?? @"C:\ProgramData\MultiSeat\library-grants.json";
    private List<Grant> Load() => File.Exists(_path)
        ? JsonSerializer.Deserialize<List<Grant>>(File.ReadAllText(_path), _json) ?? [] : [];
    private void Save(List<Grant> grants)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(_path)!);
        var temp = _path + "." + Guid.NewGuid().ToString("N") + ".tmp";
        try { File.WriteAllText(temp, JsonSerializer.Serialize(grants, _json)); File.Move(temp, _path, true); }
        finally { if (File.Exists(temp)) File.Delete(temp); }
    }
    private static FileSystemAccessRule Rule(string sid) => new(new SecurityIdentifier(sid),
        FileSystemRights.ReadAndExecute, InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit,
        PropagationFlags.None, AccessControlType.Allow);

    internal void Add(GameEntry game, string account)
    {
        var directory = !string.IsNullOrWhiteSpace(game.ExePath)
            ? Path.GetDirectoryName(Path.GetFullPath(game.ExePath))!
            : game.InstallPath;
        if (!LibraryAccess.AllowedDirectory(directory))
            throw new AdminFault("invalid_params", "Dossier protégé : choisissez le dossier du jeu, pas une racine ou un dossier personnel");
        if (!Directory.Exists(directory)) throw new AdminFault("invalid_params", "Dossier du jeu introuvable");
        // A junction below an accepted path must not point into a protected tree.
        var current = Path.GetPathRoot(directory)!;
        foreach (var component in directory[current.Length..].Split(['\\', '/'], StringSplitOptions.RemoveEmptyEntries))
        {
            current = Path.Combine(current, component);
            if ((File.GetAttributes(current) & FileAttributes.ReparsePoint) != 0)
                throw new AdminFault("invalid_params", "Les dossiers de jeu avec jonction ou lien ne sont pas autorisés");
        }
        lock (_gate)
        {
            var grants = Load();
            if (grants.Any(g => g.GameId.Equals(game.Id, StringComparison.OrdinalIgnoreCase) &&
                g.Account.Equals(account, StringComparison.OrdinalIgnoreCase))) return;
            var sid = ((SecurityIdentifier)new NTAccount(Environment.MachineName, account)
                .Translate(typeof(SecurityIdentifier))).Value;
            var shared = grants.FirstOrDefault(g => g.Sid == sid &&
                g.Directory.Equals(directory, StringComparison.OrdinalIgnoreCase));
            if (shared is not null)
            {
                grants.Add(new(game.Id, account, directory, sid, false));
                Save(grants);
                return;
            }
            var info = new DirectoryInfo(directory);
            var acl = info.GetAccessControl();
            var rule = Rule(sid);
            var existing = acl.GetAccessRules(true, false, typeof(SecurityIdentifier))
                .OfType<FileSystemAccessRule>().Any(r => r.IdentityReference.Value == sid &&
                    r.AccessControlType == AccessControlType.Allow && r.FileSystemRights == rule.FileSystemRights &&
                    r.InheritanceFlags == rule.InheritanceFlags && r.PropagationFlags == rule.PropagationFlags);
            if (!existing)
            {
                acl.AddAccessRule(rule);
                info.SetAccessControl(acl);
            }
            try { grants.Add(new(game.Id, account, directory, sid, !existing)); Save(grants); }
            catch
            {
                if (!existing)
                {
                    acl.RemoveAccessRuleSpecific(rule);
                    info.SetAccessControl(acl);
                }
                throw;
            }
        }
    }

    internal void Remove(string? gameId, string? account)
    {
        lock (_gate)
        {
            var grants = Load();
            var selected = grants.Where(g => (gameId is null || g.GameId.Equals(gameId, StringComparison.OrdinalIgnoreCase)) &&
                         (account is null || g.Account.Equals(account, StringComparison.OrdinalIgnoreCase))).ToArray();
            foreach (var grant in selected)
            {
                var remaining = grants.Where(g => !selected.Contains(g) && g.Sid == grant.Sid &&
                    g.Directory.Equals(grant.Directory, StringComparison.OrdinalIgnoreCase)).ToArray();
                if (remaining.Length > 0)
                {
                    if (grant.OwnsRule)
                    {
                        var index = grants.IndexOf(remaining[0]);
                        grants[index] = remaining[0] with { OwnsRule = true };
                    }
                    continue;
                }
                if (!grant.OwnsRule || !Directory.Exists(grant.Directory)) continue;
                var info = new DirectoryInfo(grant.Directory);
                var acl = info.GetAccessControl();
                acl.RemoveAccessRuleSpecific(Rule(grant.Sid));
                info.SetAccessControl(acl);
            }
            grants.RemoveAll(selected.Contains);
            if (selected.Length > 0) Save(grants);
        }
    }
}
