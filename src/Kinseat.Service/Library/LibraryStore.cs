using System.IO;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using MultiSeat.Service.Ipc;

namespace MultiSeat.Service.Library;

public sealed class LibraryStore
{
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web) { WriteIndented = true };
    private readonly object _gate = new();
    private readonly LibraryGrants _grants;
    private readonly string _path;
    private List<GameEntry> _games = [];
    private DateTimeOffset? _scannedAt;
    private bool _loaded;

    public LibraryStore(string? path = null)
    {
        _path = path ?? @"C:\ProgramData\Kinseat\library.json";
        _grants = new LibraryGrants(path is null ? null : Path.Combine(Path.GetDirectoryName(Path.GetFullPath(path))!, "library-grants.json"));
    }

    public void GrantAccess(string id, string account)
    {
        lock (_gate)
        {
            Load();
            var game = _games.FirstOrDefault(g => g.Id.Equals(id, StringComparison.OrdinalIgnoreCase))
                ?? throw new AdminFault("not_found", "Jeu introuvable");
            if (!game.AssignedTo.Contains(account, StringComparer.OrdinalIgnoreCase))
                throw new AdminFault("invalid_params", "Ce compte n'est pas autorisé pour ce jeu");
            _grants.Add(game, account);
        }
    }

    private void Load()
    {
        if (_loaded) return;
        if (File.Exists(_path))
        {
            var saved = JsonSerializer.Deserialize<LibrarySnapshot>(File.ReadAllText(_path), Json);
            _games = saved?.Games ?? [];
            _scannedAt = saved?.ScannedAt;
        }
        _loaded = true;
    }

    private LibrarySnapshot Snapshot(bool includeAccess = true)
    {
        var games = _games.Select(Clone).ToList();
        if (includeAccess)
            foreach (var game in games)
            {
                game.Access = game.AssignedTo.Distinct(StringComparer.OrdinalIgnoreCase)
                    .ToDictionary(a => a, a => LibraryAccess.Status(game, a), StringComparer.OrdinalIgnoreCase);
                game.Store = game.Source;
                // Cloud is only known from the store; profile saves are not detected yet.
                // A manually added exe can be redirected only when isolation is enabled and safe.
                game.SaveLocation = game.Source.ToLowerInvariant() switch
                {
                    "steam" => "cloud:Steam",
                    "epic" => "cloud:Epic",
                    _ when game.ExePath is not null && game.LaunchUri is null && game.SaveIsolation == "auto" && !game.AntiCheat => "redirected",
                    _ => "unknown"
                };
            }
        return new() { Games = games, ScannedAt = _scannedAt };
    }
    private static GameEntry Clone(GameEntry g) => new()
    {
        Id = g.Id, Name = g.Name, Source = g.Source, InstallPath = g.InstallPath,
        ExePath = g.ExePath, LaunchUri = g.LaunchUri, SizeBytes = g.SizeBytes,
        SaveIsolation = g.SaveIsolation, StartFromHostSave = g.StartFromHostSave, AntiCheat = g.AntiCheat,
        AssignedTo = new List<string>(g.AssignedTo)
    };

    public LibrarySnapshot Get()
    {
        lock (_gate) { Load(); return Snapshot(); }
    }

    public LibrarySnapshot Merge(IEnumerable<GameEntry> scanned, IEnumerable<string>? failedSources = null)
    {
        lock (_gate)
        {
            Load();
            var failed = new HashSet<string>(failedSources ?? [], StringComparer.OrdinalIgnoreCase);
            var old = _games.ToDictionary(g => g.Id, StringComparer.OrdinalIgnoreCase);
            var next = scanned.Where(g => g.Source != "manual").DistinctBy(g => g.Id, StringComparer.OrdinalIgnoreCase)
                .Select(g =>
                {
                    var copy = Clone(g);
                    if (old.TryGetValue(g.Id, out var existing))
                    {
                        copy.AssignedTo = new List<string>(existing.AssignedTo);
                        copy.SaveIsolation = existing.SaveIsolation;
                        copy.StartFromHostSave = existing.StartFromHostSave;
                    }
                    copy.AntiCheat = AntiCheatDetector.Detect(copy.InstallPath);
                    return copy;
                }).ToList();
            next.AddRange(_games.Where(g => g.Source == "manual" || failed.Contains(g.Source)).Where(g => next.All(n => !string.Equals(n.Id, g.Id, StringComparison.OrdinalIgnoreCase))).Select(Clone));
            foreach (var game in next.Where(g => g.Source == "manual"))
                game.AntiCheat = AntiCheatDetector.Detect(game.InstallPath);
            _games = next;
            _scannedAt = DateTimeOffset.UtcNow;
            Save();
            return Snapshot();
        }
    }

    public static string ManualId(string exePath)
    {
        var normalized = Path.GetFullPath(exePath).Replace('/', '\\').ToLowerInvariant();
        return "manual:" + Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(normalized)))[..16].ToLowerInvariant();
    }

    public LibrarySnapshot Add(string name, string exePath)
    {
        lock (_gate)
        {
            Load();
            var fullPath = Path.GetFullPath(exePath);
            var id = ManualId(fullPath);
            if (_games.All(g => !string.Equals(g.Id, id, StringComparison.OrdinalIgnoreCase)))
                _games.Add(new GameEntry { Id = id, Name = name, Source = "manual", InstallPath = Path.GetDirectoryName(fullPath)!, ExePath = fullPath,
                    AntiCheat = AntiCheatDetector.Detect(Path.GetDirectoryName(fullPath)!) });
            Save(); return Snapshot();
        }
    }

    public LibrarySnapshot SetOptions(string id, string? saveIsolation, bool? startFromHostSave)
    {
        if (saveIsolation is not null && saveIsolation is not ("auto" or "off"))
            throw new AdminFault("invalid_params", "saveIsolation doit valoir auto ou off");
        lock (_gate)
        {
            Load();
            var game = _games.FirstOrDefault(g => string.Equals(g.Id, id, StringComparison.OrdinalIgnoreCase))
                ?? throw new AdminFault("not_found", "Jeu introuvable");
            if (saveIsolation is not null) game.SaveIsolation = saveIsolation;
            if (startFromHostSave.HasValue) game.StartFromHostSave = startFromHostSave.Value;
            Save(); return Snapshot();
        }
    }

    public LibrarySnapshot Remove(string id)
    {
        lock (_gate)
        {
            Load();
            var game = _games.FirstOrDefault(g => string.Equals(g.Id, id, StringComparison.OrdinalIgnoreCase))
                ?? throw new AdminFault("not_found", "Jeu introuvable");
            if (game.Source != "manual") throw new AdminFault("invalid_params", "Seuls les jeux ajoutés manuellement peuvent être supprimés");
            _grants.Remove(id, null);
            _games.Remove(game); Save(); return Snapshot();
        }
    }

    /// <summary>Remove an account from every game's allowed list (seat deletion). Returns how many games changed.</summary>
    public int RevokeAccount(string accountName)
    {
        lock (_gate)
        {
            Load();
            _grants.Remove(null, accountName);
            var changed = _games.Sum(g => g.AssignedTo.RemoveAll(a => string.Equals(a, accountName, StringComparison.OrdinalIgnoreCase)) > 0 ? 1 : 0);
            if (changed > 0) Save();
            return changed;
        }
    }

    public LibrarySnapshot Assign(string id, string accountName, bool allowed)
    {
        lock (_gate)
        {
            Load();
            var game = _games.FirstOrDefault(g => string.Equals(g.Id, id, StringComparison.OrdinalIgnoreCase))
                ?? throw new AdminFault("not_found", "Jeu introuvable");
            if (!allowed) _grants.Remove(id, accountName);
            game.AssignedTo.RemoveAll(a => string.Equals(a, accountName, StringComparison.OrdinalIgnoreCase));
            if (allowed) game.AssignedTo.Add(accountName);
            Save(); return Snapshot();
        }
    }

    private void Save()
    {
        var dir = Path.GetDirectoryName(Path.GetFullPath(_path))!;
        Directory.CreateDirectory(dir);
        var temp = Path.Combine(dir, ".library-" + Guid.NewGuid().ToString("N") + ".tmp");
        try
        {
            File.WriteAllText(temp, JsonSerializer.Serialize(Snapshot(includeAccess: false), Json));
            File.Move(temp, _path, true);
        }
        finally { if (File.Exists(temp)) File.Delete(temp); }
    }
}

public sealed class LibrarySnapshot
{
    public List<GameEntry> Games { get; set; } = [];
    public DateTimeOffset? ScannedAt { get; set; }
}

public sealed class LibraryIpcHandler(LibraryStore store, GameScannerRunner scanners, GameShortcuts? shortcuts = null, GameLaunchManifestWriter? manifests = null) : IAdminMethodHandler
{
    public static readonly string[] Methods = ["library.list", "library.scan", "library.add", "library.remove", "library.assign", "library.grantAccess", "library.setOptions"];
    IReadOnlyCollection<string> IAdminMethodHandler.Methods => Methods;
    public event Action<string>? AssignmentsChanged;

    public Task<object> HandleAsync(string method, JsonElement parameters, CancellationToken ct)
    {
        ct.ThrowIfCancellationRequested();
        LibrarySnapshot result;
        switch (method)
        {
            case "library.list":
                result = store.Get();
                if (result.ScannedAt is null) result = Scan();
                break;
            case "library.scan": result = Scan(); break;
            case "library.setOptions":
                if (parameters.ValueKind != JsonValueKind.Object ||
                    parameters.EnumerateObject().Select(p => p.Name).Distinct(StringComparer.Ordinal).Count() != parameters.EnumerateObject().Count() ||
                    parameters.EnumerateObject().Any(p => p.Name is not ("gameId" or "saveIsolation" or "startFromHostSave")))
                    throw new AdminFault("invalid_params", "Paramètres de bibliothèque invalides");
                var gameId = Required(parameters, "gameId");
                string? isolation = null;
                if (parameters.TryGetProperty("saveIsolation", out var mode))
                {
                    if (mode.ValueKind != JsonValueKind.String || mode.GetString() is not ("auto" or "off"))
                        throw new AdminFault("invalid_params", "saveIsolation doit valoir auto ou off");
                    isolation = mode.GetString();
                }
                bool? fromHost = null;
                if (parameters.TryGetProperty("startFromHostSave", out var host))
                {
                    if (host.ValueKind is not (JsonValueKind.True or JsonValueKind.False))
                        throw new AdminFault("invalid_params", "startFromHostSave doit être un booléen");
                    fromHost = host.GetBoolean();
                }
                result = store.SetOptions(gameId, isolation, fromHost);
                var changedGame = result.Games.First(g => string.Equals(g.Id, gameId, StringComparison.OrdinalIgnoreCase));
                foreach (var assigned in changedGame.AssignedTo.Distinct(StringComparer.OrdinalIgnoreCase))
                {
                    try
                    {
                        manifests?.Sync(assigned, result.Games);
                    }
                    catch (IOException) { }
                    catch (UnauthorizedAccessException) { }
                }
                break;
            case "library.add":
                var name = Required(parameters, "name");
                var exe = Required(parameters, "exePath");
                if (!File.Exists(exe)) throw new AdminFault("invalid_params", "L'exécutable indiqué n'existe pas");
                result = store.Add(name, exe);
                foreach (var assigned in result.Games.First(g => g.Id == LibraryStore.ManualId(exe)).AssignedTo)
                    shortcuts?.Sync(assigned, result.Games);
                break;
            case "library.remove":
                var removed = Required(parameters, "id");
                var old = store.Get().Games.FirstOrDefault(g => g.Id.Equals(removed, StringComparison.OrdinalIgnoreCase));
                result = store.Remove(removed);
                if (old is not null) foreach (var assigned in old.AssignedTo)
                {
                    shortcuts?.Sync(assigned, result.Games);
                    AssignmentsChanged?.Invoke(assigned);
                }
                break;
            case "library.grantAccess":
                store.GrantAccess(Required(parameters, "gameId"), Required(parameters, "accountName"));
                result = store.Get();
                break;
            case "library.assign":
                var id = Required(parameters, "gameId");
                var account = Required(parameters, "accountName");
                if (parameters.ValueKind != JsonValueKind.Object || !parameters.TryGetProperty("allowed", out var allowed)
                    || allowed.ValueKind is not (JsonValueKind.True or JsonValueKind.False))
                    throw new AdminFault("invalid_params", "Le champ allowed doit être un booléen");
                result = store.Assign(id, account, allowed.GetBoolean());
                shortcuts?.Sync(account, result.Games);
                AssignmentsChanged?.Invoke(account);
                break;
            default: throw new AdminFault("invalid_params", "Méthode de bibliothèque inconnue");
        }
        return Task.FromResult<object>(result);
    }

    private LibrarySnapshot Scan()
    {
        var (games, failed) = scanners.Scan();
        return store.Merge(games, failed);
    }

    private static string Required(JsonElement parameters, string key)
    {
        if (parameters.ValueKind != JsonValueKind.Object || !parameters.TryGetProperty(key, out var value)
            || value.ValueKind != JsonValueKind.String || string.IsNullOrWhiteSpace(value.GetString()))
            throw new AdminFault("invalid_params", $"Champ {key} manquant ou invalide");
        return value.GetString()!;
    }
}
