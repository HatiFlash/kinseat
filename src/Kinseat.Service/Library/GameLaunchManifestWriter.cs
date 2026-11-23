using System.Security.AccessControl;
using System.Security.Principal;
using System.Text;
using System.Text.Json;
using Microsoft.Win32;
using Microsoft.Extensions.Logging.Abstractions;
using MultiSeat.Service.Accounts;
using MultiSeat.Service.Configuration;

namespace MultiSeat.Service.Library;

/// <summary>Prepares the per-account, read-only game-file isolation contract.</summary>
public sealed class GameLaunchManifestWriter
{
    private readonly Func<string, bool> _eligible;
    private readonly Func<string, (string Sid, string Profile)?> _identity;
    private readonly string _root;
    private readonly string _publicDocuments;
    private readonly Func<string, bool> _fileExists;
    private readonly Func<string, bool> _isPortable;
    private readonly ILogger<GameLaunchManifestWriter> _logger;
    private int _missingLauncherWarned;
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web);

    public GameLaunchManifestWriter(AccountManager accounts, ILogger<GameLaunchManifestWriter> logger)
        : this(account => accounts.IsManagedSeatAccount(account),
            ResolveIdentity, @"C:\ProgramData\MultiSeat\games", @"C:\Users\Public\Documents", logger: logger) { }

    internal GameLaunchManifestWriter(Func<string, bool> eligible,
        Func<string, (string Sid, string Profile)?> identity, string root, string publicDocuments,
        Func<string, bool>? fileExists = null, ILogger<GameLaunchManifestWriter>? logger = null,
        Func<string, bool>? isPortable = null)
    {
        _isPortable = isPortable ?? IsPortable;
        _eligible = eligible;
        _identity = identity;
        _root = root;
        _publicDocuments = publicDocuments;
        _fileExists = fileExists ?? File.Exists;
        _logger = logger ?? NullLogger<GameLaunchManifestWriter>.Instance;
    }

    private static (string Sid, string Profile)? ResolveIdentity(string account)
    {
        var sid = new NTAccount(Environment.MachineName, account).Translate(typeof(SecurityIdentifier)).Value;
        using var key = Registry.LocalMachine.OpenSubKey(
            $@"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\{sid}");
        return key?.GetValue("ProfileImagePath") is string path
            ? (sid, Environment.ExpandEnvironmentVariables(path)) : null;
    }

    // Library IDs contain ':'; escape every non-alphanumeric character to avoid path traversal and collisions.
    internal static string FileName(string id) => string.Concat(Encoding.UTF8.GetBytes(id).Select(b =>
        b is >= (byte)'a' and <= (byte)'z' or >= (byte)'A' and <= (byte)'Z' or >= (byte)'0' and <= (byte)'9'
            ? ((char)b).ToString() : $"%{b:X2}")) + ".json";

    internal string ManifestPath(string account, string id) => Path.Combine(_root, account, FileName(id));

    internal static string LauncherPath(MultiSeatOptions options) => options.GameFilesLauncherPath;
    internal Func<string, bool> FileExistsCheck => _fileExists;

    internal static bool LauncherFilesExist(string launcher, Func<string, bool> fileExists) =>
        !string.IsNullOrWhiteSpace(launcher) && fileExists(launcher) &&
        fileExists(Path.Combine(Path.GetDirectoryName(launcher)!, "Kinseat.GameFiles.dll"));

    internal bool LauncherAvailable(MultiSeatOptions options)
    {
        var launcher = LauncherPath(options);
        if (LauncherFilesExist(launcher, _fileExists)) return true;
        if (Interlocked.Exchange(ref _missingLauncherWarned, 1) == 0)
            _logger.LogWarning("GameFiles launcher or Kinseat.GameFiles.dll missing beside {Launcher}; games will start directly without a manifest", launcher);
        return false;
    }

    internal static string Command(string launcher, string manifest, string exe) =>
        $"\"{launcher}\" --game \"{manifest}\" -- \"{exe}\"";

    internal static bool IsPortable(string exe)
    {
        var path = exe.Replace('/', '\\');
        return !new[] { @"C:\Program Files\", @"C:\Program Files (x86)\", @"C:\Program Files\WindowsApps\",
            @"C:\Windows\", @"C:\Program Files\ModifiableWindowsApps\" }
            .Any(prefix => path.StartsWith(prefix, StringComparison.OrdinalIgnoreCase));
    }

    internal static IReadOnlyList<string> FindSaveDirectories(string gameDir)
    {
        string[] names = ["save", "saves", "savegame", "savegames", "savedata", "save data", "saved games"];
        var found = new List<string>();
        var pending = new Queue<(string Path, int Depth)>();
        pending.Enqueue((gameDir, 0));
        var visited = 0;
        while (pending.Count > 0 && visited < 2000)
        {
            var (parent, depth) = pending.Dequeue();
            if (depth >= 3) continue;
            string[] children;
            try { children = Directory.GetDirectories(parent); }
            catch (UnauthorizedAccessException) { continue; }
            catch (IOException) { continue; }
            Array.Sort(children, StringComparer.OrdinalIgnoreCase);
            foreach (var child in children)
            {
                if (++visited > 2000) break;
                FileAttributes attributes;
                try { attributes = File.GetAttributes(child); }
                catch (UnauthorizedAccessException) { continue; }
                catch (IOException) { continue; }
                if ((attributes & FileAttributes.ReparsePoint) != 0) continue;
                if (names.Contains(Path.GetFileName(child), StringComparer.OrdinalIgnoreCase))
                    found.Add(child);
                else
                    pending.Enqueue((child, depth + 1));
            }
        }
        found.Sort(StringComparer.OrdinalIgnoreCase);
        return found;
    }

    internal static bool IsIsolatable(GameEntry game) =>
        !string.IsNullOrWhiteSpace(game.ExePath) && string.IsNullOrWhiteSpace(game.LaunchUri) &&
        game.SaveIsolation == "auto" && !game.AntiCheat && game.Source != "browser";

    /// <summary>Only an exact, immediate directory matching the game name is identifiable.
    /// No generic Steam/RUNE or other shared folder is imported; no symlink is followed.</summary>
    internal static void CopyIdentifiedHostSave(string publicDocuments, string name, string destination)
    {
        if (string.IsNullOrWhiteSpace(name) || name is "." or ".." || name.IndexOfAny(['/', '\\', ':']) >= 0) return;
        if (!Directory.Exists(publicDocuments)) return;
        var source = Directory.EnumerateDirectories(publicDocuments).FirstOrDefault(dir =>
            string.Equals(Path.GetFileName(dir), name, StringComparison.OrdinalIgnoreCase) &&
            (File.GetAttributes(dir) & FileAttributes.ReparsePoint) == 0);
        if (source is null) return;
        void Copy(string from, string to)
        {
            Directory.CreateDirectory(to);
            foreach (var file in Directory.EnumerateFiles(from))
            {
                if ((File.GetAttributes(file) & FileAttributes.ReparsePoint) != 0) continue;
                var target = Path.Combine(to, Path.GetFileName(file));
                if (!File.Exists(target)) File.Copy(file, target);
            }
            foreach (var dir in Directory.EnumerateDirectories(from))
                if ((File.GetAttributes(dir) & FileAttributes.ReparsePoint) == 0)
                    Copy(dir, Path.Combine(to, Path.GetFileName(dir)));
        }
        Copy(source, Path.Combine(destination, Path.GetFileName(source)));
    }

    private static DirectorySecurity DirectoryAcl(string sid, bool playerWrite)
    {
        var acl = new DirectorySecurity();
        acl.SetAccessRuleProtection(true, false);
        foreach (var (identity, rights) in new[] {
            (new SecurityIdentifier(WellKnownSidType.LocalSystemSid, null), FileSystemRights.FullControl),
            (new SecurityIdentifier(WellKnownSidType.BuiltinAdministratorsSid, null), FileSystemRights.FullControl),
            (new SecurityIdentifier(sid), playerWrite ? FileSystemRights.Modify : FileSystemRights.ReadAndExecute) })
            acl.AddAccessRule(new FileSystemAccessRule(identity, rights,
                InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit, PropagationFlags.None, AccessControlType.Allow));
        return acl;
    }

    private static void SecureDirectory(string path, string sid, bool write)
    {
        // A seat may pre-create a junction under ProgramData or its profile. Never ACL its target.
        var current = Path.GetPathRoot(path)!;
        foreach (var part in path[current.Length..].Split(['\\', '/'], StringSplitOptions.RemoveEmptyEntries))
        {
            current = Path.Combine(current, part);
            if (Directory.Exists(current) && (File.GetAttributes(current) & FileAttributes.ReparsePoint) != 0)
                throw new IOException("Dossier redirigé : préparation du jeu refusée");
        }
        Directory.CreateDirectory(path);
        new DirectoryInfo(path).SetAccessControl(DirectoryAcl(sid, write));
    }

    private static void SecureFile(string path, string sid, bool playerWrite)
    {
        var acl = new FileSecurity();
        acl.SetAccessRuleProtection(true, false);
        foreach (var (identity, rights) in new[] {
            (new SecurityIdentifier(WellKnownSidType.LocalSystemSid, null), FileSystemRights.FullControl),
            (new SecurityIdentifier(WellKnownSidType.BuiltinAdministratorsSid, null), FileSystemRights.FullControl),
            (new SecurityIdentifier(sid), playerWrite ? FileSystemRights.Modify : FileSystemRights.Read) })
            acl.AddAccessRule(new FileSystemAccessRule(identity, rights, AccessControlType.Allow));
        new FileInfo(path).SetAccessControl(acl);
    }

    public void Clear(string account)
    {
        if (string.IsNullOrWhiteSpace(account) || account is "." or ".." ||
            account.Contains("..", StringComparison.Ordinal) || account.IndexOfAny(['/', '\\', ':']) >= 0 ||
            account.Any(char.IsControl))
            throw new ArgumentException("Nom de compte invalide", nameof(account));

        var root = Path.GetFullPath(_root);
        var directory = Path.GetFullPath(Path.Combine(root, account));
        var childPrefix = Path.TrimEndingDirectorySeparator(root) + Path.DirectorySeparatorChar;
        if (!directory.StartsWith(childPrefix, StringComparison.OrdinalIgnoreCase))
            throw new IOException("Dossier de manifestes hors du répertoire configuré");
        if (!Directory.Exists(directory)) return;
        if ((File.GetAttributes(directory) & FileAttributes.ReparsePoint) != 0)
            throw new IOException("Dossier de manifestes redirigé");
        Directory.Delete(directory, recursive: true);
    }

    public IReadOnlyDictionary<string, string> Sync(string account, IEnumerable<GameEntry> games)
    {
        var result = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        if (string.IsNullOrWhiteSpace(account) || account is "." or ".." ||
            account.IndexOfAny(['/', '\\', ':']) >= 0 || !_eligible(account) || _identity(account) is not { } identity)
            return result;
        var (sid, profile) = identity;
        if (string.IsNullOrWhiteSpace(profile) || !Directory.Exists(profile)) return result;
        var folder = Path.Combine(_root, account);
        SecureDirectory(folder, sid, false);
        foreach (var game in games.Where(g => g.AssignedTo.Contains(account, StringComparer.OrdinalIgnoreCase) && IsIsolatable(g)))
        {
            var save = Path.Combine(profile, "Documents", "Kinseat", "Sauvegardes");
            var local = Path.Combine(profile, "AppData", "Local", "Kinseat");
            var overlay = Path.Combine(local, "Jeux", FileName(game.Id)[..^5]);
            var logs = Path.Combine(local, "Journaux");
            SecureDirectory(save, sid, true);
            SecureDirectory(logs, sid, true);
            if (_isPortable(game.ExePath!)) SecureDirectory(overlay, sid, true);
            var marker = Path.Combine(folder, FileName(game.Id)[..^5] + ".kinseat-copied");
            if (game.StartFromHostSave && !File.Exists(marker))
            {
                CopyIdentifiedHostSave(_publicDocuments, game.Name, save);
                File.WriteAllText(marker, "");
                SecureFile(marker, sid, false);
            }
            var redirects = new List<object> { new { from = _publicDocuments, to = save, mode = "redirect" } };
            if (_isPortable(game.ExePath!))
            {
                if (!game.StartFromHostSave)
                {
                    var gameDir = Path.GetDirectoryName(game.ExePath)!;
                    foreach (var saveDirectory in FindSaveDirectories(gameDir))
                    {
                        var relative = Path.GetRelativePath(gameDir, saveDirectory);
                        var target = Path.Combine(local, "Jeux", FileName(game.Id)[..^5] + ".sauvegardes", relative);
                        SecureDirectory(target, sid, true);
                        redirects.Add(new { from = saveDirectory, to = target, mode = "redirect" });
                    }
                }
                redirects.Add(new { from = Path.GetDirectoryName(game.ExePath)!, to = overlay, mode = "overlay" });
            }
            var path = ManifestPath(account, game.Id);
            var manifest = new { gameId = game.Id, account, accountSid = sid, profileDir = profile,
                redirects, copyFromHostOnce = game.StartFromHostSave,
                logFile = Path.Combine(logs, "gamefiles-" + FileName(game.Id)[..^5] + ".log") };
            // The destination object receives its own protected descriptor after every replacement.
            var temp = Path.Combine(folder, ".manifest-" + Guid.NewGuid().ToString("N"));
            try
            {
                File.WriteAllText(temp, JsonSerializer.Serialize(manifest, Json), new UTF8Encoding(false));
                SecureFile(temp, sid, false);
                File.Move(temp, path, true);
            }
            finally { if (File.Exists(temp)) File.Delete(temp); }
            result.Add(game.Id, path);
        }
        foreach (var path in Directory.EnumerateFiles(folder, "*.json"))
            if (!result.Values.Contains(path, StringComparer.OrdinalIgnoreCase)) File.Delete(path);
        return result;
    }
}
