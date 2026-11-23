using System.Security.Cryptography;
using System.Security.Principal;
using System.Text;
using System.Text.Json;
using Microsoft.Win32;
using Microsoft.Extensions.Options;
using MultiSeat.Service.Accounts;
using MultiSeat.Service.Configuration;
using MultiSeat.Service.SteamShim;
using MultiSeat.Service.Streaming;

namespace MultiSeat.Service.Library;

/// <summary>Synchronise uniquement les liens dont le chemin et la cible figurent dans notre inventaire.</summary>
public sealed class GameShortcuts
{
    private readonly IShortcutStore _links;
    private readonly Func<string, string?> _profile;
    private readonly Func<string> _steamExe;
    private readonly string _inventoryDir;
    private readonly MultiSeatOptions _options;
    private readonly Func<string, bool> _browserEnabled;
    private readonly Func<string, string?> _browserExe;
    private readonly ILogger<GameShortcuts> _logger;
    private readonly GameLaunchManifestWriter? _gameManifests;
    private readonly object _gate = new();
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web) { WriteIndented = true };

    public GameShortcuts(IOptions<MultiSeatOptions> options, ILogger<GameShortcuts> logger,
        SeatPresetStore presets, AccountManager accounts, GameLaunchManifestWriter gameManifests)
        : this(new WScriptShortcutStore(), ResolveProfile, Streaming.ApolloConfigBuilder.ResolveSteamExecutable,
            @"C:\ProgramData\MultiSeat\shortcuts", options.Value, logger,
            account => accounts.PurgeEligibility(account, "S-1-0-0") is null &&
                       presets.GetByAccount(account)?.Lightening is { EmptyDesktop: true, DesktopBrowser: true },
            DesktopBrowserResolver.Resolve, gameManifests) { }

    internal GameShortcuts(IShortcutStore links, Func<string, string?> profile, Func<string> steamExe,
        string inventoryDir, MultiSeatOptions options, ILogger<GameShortcuts> logger,
        Func<string, bool>? browserEnabled = null, Func<string, string?>? browserExe = null,
        GameLaunchManifestWriter? gameManifests = null)
    {
        _links = links;
        _profile = profile;
        _steamExe = steamExe;
        _inventoryDir = inventoryDir;
        _options = options;
        _logger = logger;
        _gameManifests = gameManifests;
        _browserEnabled = browserEnabled ?? (_ => false);
        _browserExe = browserExe ?? DesktopBrowserResolver.Resolve;
    }

    private static string? ResolveProfile(string account)
    {
        // Never guess C:\Users\name: only Windows' SID-to-profile mapping is authoritative.
        var sid = new NTAccount(Environment.MachineName, account).Translate(typeof(SecurityIdentifier)).Value;
        using var key = Registry.LocalMachine.OpenSubKey(
            $@"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\{sid}");
        return key?.GetValue("ProfileImagePath") is string path ? Environment.ExpandEnvironmentVariables(path) : null;
    }

    internal sealed record OwnedLink(string Path, string Target, string GameId);
    internal sealed record ChangePlan(IReadOnlyList<OwnedLink> Remove, IReadOnlyList<(OwnedLink Link, ShortcutInfo Info)> Add);

    internal static string SafeName(string name)
    {
        var cleaned = new string(name.Select(c => c < 32 || "<>:\"/\\|?*".Contains(c) ? '_' : c).ToArray()).Trim(' ', '.');
        if (cleaned.Length > 90) cleaned = cleaned[..90].TrimEnd(' ', '.');
        if (cleaned.Length == 0) cleaned = "Jeu";
        var stem = cleaned.Split('.')[0];
        if (stem.Equals("CON", StringComparison.OrdinalIgnoreCase) || stem.Equals("PRN", StringComparison.OrdinalIgnoreCase) ||
            stem.Equals("AUX", StringComparison.OrdinalIgnoreCase) || stem.Equals("NUL", StringComparison.OrdinalIgnoreCase) ||
            (stem.Length == 4 && (stem.StartsWith("COM", StringComparison.OrdinalIgnoreCase) || stem.StartsWith("LPT", StringComparison.OrdinalIgnoreCase)) && stem[3] is >= '1' and <= '9'))
            cleaned = "_" + cleaned;
        return cleaned;
    }

    internal static ChangePlan Plan(string desktop, IEnumerable<GameEntry> games, string account,
        IReadOnlyList<OwnedLink> owned, Func<GameEntry, ShortcutInfo?> describe, Func<string, bool>? occupied = null)
    {
        var desired = new List<(OwnedLink, ShortcutInfo)>();
        var used = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var game in games.Where(g => g.AssignedTo.Contains(account, StringComparer.OrdinalIgnoreCase)).OrderBy(g => g.Id, StringComparer.OrdinalIgnoreCase))
        {
            var info = describe(game);
            if (info is null) continue;
            var stem = SafeName(game.Name);
            var path = Path.Combine(desktop, stem + ".lnk");
            var collision = used.Contains(path) || (occupied?.Invoke(path) == true &&
                !owned.Any(o => o.GameId == game.Id && string.Equals(o.Path, path, StringComparison.OrdinalIgnoreCase)));
            if (collision)
            {
                var suffix = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(game.Id)))[..8];
                path = Path.Combine(desktop, stem + "-" + suffix + ".lnk");
                for (var index = 2; used.Contains(path) || (occupied?.Invoke(path) == true &&
                     !owned.Any(o => o.GameId == game.Id && string.Equals(o.Path, path, StringComparison.OrdinalIgnoreCase))); index++)
                    path = Path.Combine(desktop, stem + "-" + suffix + "-" + index + ".lnk");
            }
            used.Add(path);
            desired.Add((new OwnedLink(path, info.Target, game.Id), info));
        }
        var keep = desired.Where(d => owned.Any(o => o == d.Item1)).Select(d => d.Item1).ToHashSet();
        return new ChangePlan(owned.Where(o => !keep.Contains(o)).ToList(), desired.Where(d => !keep.Contains(d.Item1)).ToList());
    }

    internal ShortcutInfo? Describe(GameEntry game, IReadOnlyDictionary<string, string>? manifests = null)
    {
        if (GameLaunchManifestWriter.IsIsolatable(game) &&
            manifests?.TryGetValue(game.Id, out var manifest) == true &&
            _gameManifests?.LauncherAvailable(_options) == true)
        {
            var launcher = GameLaunchManifestWriter.LauncherPath(_options);
            return new ShortcutInfo(launcher, $"--game \"{manifest}\" -- \"{game.ExePath}\"",
                Path.GetDirectoryName(game.ExePath) ?? "", game.ExePath!, game.Name);
        }
        if (GameLaunchManifestWriter.IsIsolatable(game))
        {
            // Fail closed: a direct shortcut would let the game write into the host's game folder.
            _logger.LogWarning("Raccourci {Game} non créé : isolation des fichiers indisponible (manifeste ou lanceur absent)", game.Name);
            return null;
        }
        if (game.Source == "steam" && _options.SteamIsolationEnabled &&
            game.LaunchUri?.StartsWith("steam://", StringComparison.OrdinalIgnoreCase) == true)
        {
            var json = JsonSerializer.Serialize(new { apps = new[] { new { name = game.Name, detached = new[] { game.LaunchUri } } } });
            var rewritten = SteamAppListRewriter.Rewrite(json, _options.SteamIsolationLauncherPath,
                _steamExe(), steam =>
                {
                    try { return Sessions.SteamIsolationLaunchPlan.Create(steam, null, _options).Supervised ? null : "Steam non isolé"; }
                    catch (InvalidOperationException ex) { return ex.Message; }
                }, out _, out var refused);
            if (refused.Count != 0)
            {
                _logger.LogWarning("Raccourci Steam {Game} refusé : {Reason}", game.Name, refused[0].Reason);
                return null;
            }
            using var doc = JsonDocument.Parse(rewritten);
            var cmd = doc.RootElement.GetProperty("apps")[0].GetProperty("cmd").GetString()!;
            var prefix = "\"" + _options.SteamIsolationLauncherPath + "\" ";
            if (!cmd.StartsWith(prefix, StringComparison.Ordinal)) return null;
            return new ShortcutInfo(_options.SteamIsolationLauncherPath, cmd[prefix.Length..], "", "", game.Name);
        }
        if (!string.IsNullOrWhiteSpace(game.LaunchUri))
            return new ShortcutInfo("explorer.exe", "\"" + game.LaunchUri + "\"", "", game.ExePath ?? "", game.Name);
        if (string.IsNullOrWhiteSpace(game.ExePath)) return null;
        return new ShortcutInfo(game.ExePath, "", Path.GetDirectoryName(game.ExePath) ?? "", game.ExePath, game.Name);
    }

    public void Sync(string account, IEnumerable<GameEntry> games, bool cleanup = false)
    {
        try { lock (_gate) SyncCore(account, games, cleanup); }
        catch (Exception ex) { _logger.LogWarning(ex, "Impossible de synchroniser les raccourcis de {Account}", account); }
    }

    /// <summary>Deletion reports failures instead of hiding them; used before profile purge.</summary>
    public void Clear(string account)
    {
        lock (_gate) SyncCore(account, [], cleanup: true);
    }

    private void SyncCore(string account, IEnumerable<GameEntry> games, bool cleanup)
    {
        if (string.IsNullOrWhiteSpace(account) || account.IndexOfAny(['/', '\\', ':']) >= 0 || account is "." or "..")
            throw new IOException("Nom de compte invalide pour l'inventaire");
        var inventory = Path.Combine(_inventoryDir, account + ".json");
        var profile = _profile(account);
        if (profile is null || !Directory.Exists(profile))
        {
            // The profile may already have been purged; its inventory must not survive Clear.
            if (cleanup && File.Exists(inventory)) File.Delete(inventory);
            return;
        }
        var desktop = Path.Combine(profile, "Desktop");
        if (Directory.Exists(desktop) && (File.GetAttributes(desktop) & FileAttributes.ReparsePoint) != 0)
            throw new IOException("Bureau redirigé : raccourcis non modifiés");
        var owned = File.Exists(inventory)
            ? JsonSerializer.Deserialize<List<OwnedLink>>(File.ReadAllText(inventory), Json) ?? throw new JsonException("Inventaire vide")
            : [];
        var validOwned = owned.Where(o => IsOnDesktop(o.Path, desktop) && File.Exists(o.Path) &&
            (File.GetAttributes(o.Path) & FileAttributes.ReparsePoint) == 0 &&
            string.Equals(_links.Read(o.Path)?.Target, o.Target, StringComparison.OrdinalIgnoreCase)).ToList();
        if (!cleanup) Directory.CreateDirectory(desktop);
        var desiredGames = games.ToList();
        var manifests = _gameManifests?.Sync(account, cleanup ? [] : desiredGames);
        if (manifests is { Count: > 0 } && _gameManifests?.LauncherAvailable(_options) != true)
            manifests = null;
        if (!cleanup && _browserEnabled(account) && _browserExe(account) is { } browser)
            desiredGames.Add(new GameEntry { Id = "browser", Name = "Navigateur", Source = "browser",
                InstallPath = Path.GetDirectoryName(browser) ?? "", ExePath = browser,
                AssignedTo = [account] });
        var plan = Plan(desktop, desiredGames, account, validOwned, g => Describe(g, manifests), File.Exists);
        var next = new List<OwnedLink>();
        var created = new List<OwnedLink>();
        foreach (var old in plan.Remove)
        {
            if (!IsOnDesktop(old.Path, desktop)) continue;
            if (!File.Exists(old.Path)) continue;
            if ((File.GetAttributes(old.Path) & FileAttributes.ReparsePoint) != 0 ||
                !string.Equals(_links.Read(old.Path)?.Target, old.Target, StringComparison.OrdinalIgnoreCase)) continue;
            try { File.Delete(old.Path); }
            catch { next.Add(old); throw; }
        }
        foreach (var old in validOwned.Except(plan.Remove))
            if (IsOnDesktop(old.Path, desktop) && File.Exists(old.Path)) next.Add(old);
        try
        {
            if (!cleanup)
                foreach (var (link, info) in plan.Add)
                {
                    if (File.Exists(link.Path))
                    {
                        // A pre-existing player shortcut is never overwritten, even if its target matches.
                        _logger.LogWarning("Raccourci déjà présent, non remplacé : {Path}", link.Path);
                        continue;
                    }
                    // Track before COM writes: a partial write also needs rollback.
                    created.Add(link);
                    _links.Write(link.Path, info);
                    next.Add(link);
                }
            if (next.Count == 0)
            {
                if (File.Exists(inventory)) File.Delete(inventory);
            }
            else
            {
                Directory.CreateDirectory(_inventoryDir);
                var temp = Path.Combine(_inventoryDir, ".shortcuts-" + Guid.NewGuid().ToString("N") + ".tmp");
                try { File.WriteAllText(temp, JsonSerializer.Serialize(next, Json)); File.Move(temp, inventory, true); }
                finally { if (File.Exists(temp)) File.Delete(temp); }
            }
        }
        catch
        {
            foreach (var link in created)
                if (File.Exists(link.Path) && (File.GetAttributes(link.Path) & FileAttributes.ReparsePoint) == 0)
                    try { File.Delete(link.Path); } catch (Exception ex) { _logger.LogWarning(ex, "Raccourci incomplet à nettoyer : {Path}", link.Path); }
            throw;
        }
    }

    private static bool IsOnDesktop(string path, string desktop) =>
        string.Equals(Path.GetDirectoryName(Path.GetFullPath(path)), Path.GetFullPath(desktop), StringComparison.OrdinalIgnoreCase);
}
