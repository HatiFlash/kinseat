using System.Security.AccessControl;
using System.Security.Claims;
using System.Security.Principal;
using System.Text.Json;

namespace MultiSeat.Service.Ipc;

/// <summary>Compte Windows autorisé à ouvrir l'application Hôte sans élévation.</summary>
public sealed record AdminAuthorization(string Sid, string Name, DateTimeOffset AuthorizedAt);

/// <summary>
/// Liste des administrateurs du PC qui ont accepté une fois l'UAC de l'application Hôte.
/// Fichier protégé (SYSTEM et Administrateurs uniquement), écrit par le service seul.
/// Un SID listé n'ouvre le canal que s'il appartient encore au groupe Administrateurs.
/// </summary>
public sealed class AdminAuthorizationStore
{
    internal const string AdministratorsSid = "S-1-5-32-544";
    private const string SystemSid = "S-1-5-18";
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web) { WriteIndented = true };
    private readonly string _path;
    private readonly bool _protectFile;
    private readonly object _gate = new();
    private List<AdminAuthorization>? _cache;

    public AdminAuthorizationStore() : this(Path.Combine(@"C:\ProgramData\MultiSeat", "admin-authorizations.json"), true) { }

    internal AdminAuthorizationStore(string path, bool protectFile = false)
    {
        _path = path;
        _protectFile = protectFile;
    }

    public string FilePath => _path;

    public IReadOnlyList<AdminAuthorization> GetAll()
    {
        lock (_gate) return Load().ToArray();
    }

    public bool Contains(string? sid)
    {
        if (string.IsNullOrEmpty(sid)) return false;
        lock (_gate) return Load().Any(a => a.Sid.Equals(sid, StringComparison.OrdinalIgnoreCase));
    }

    /// <summary>Ajoute le compte ; renvoie false s'il était déjà autorisé (idempotent).</summary>
    public bool Add(string sid, string name, DateTimeOffset now)
    {
        if (!IsValidSid(sid)) throw new ArgumentException("SID invalide", nameof(sid));
        lock (_gate)
        {
            var list = Load();
            if (list.Any(a => a.Sid.Equals(sid, StringComparison.OrdinalIgnoreCase))) return false;
            list.Add(new AdminAuthorization(sid, name, now));
            Save(list);
            return true;
        }
    }

    /// <summary>Retire le compte ; renvoie false s'il n'était pas autorisé.</summary>
    public bool Remove(string sid)
    {
        lock (_gate)
        {
            var list = Load();
            if (list.RemoveAll(a => a.Sid.Equals(sid, StringComparison.OrdinalIgnoreCase)) == 0) return false;
            Save(list);
            return true;
        }
    }

    private List<AdminAuthorization> Load()
    {
        if (_cache is not null) return _cache;
        try
        {
            _cache = File.Exists(_path)
                ? (JsonSerializer.Deserialize<List<AdminAuthorization>>(File.ReadAllText(_path), Json) ?? [])
                    .Where(a => IsValidSid(a.Sid)).ToList()
                : [];
        }
        catch (Exception ex) when (ex is JsonException or IOException or UnauthorizedAccessException)
        {
            // Fichier illisible : personne n'est autorisé sans élévation (échec fermé).
            _cache = [];
        }
        return _cache;
    }

    private void Save(List<AdminAuthorization> list)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(_path)!);
        var temp = _path + ".tmp";
        File.WriteAllText(temp, JsonSerializer.Serialize(list, Json));
        if (_protectFile && OperatingSystem.IsWindows()) Protect(temp);
        File.Move(temp, _path, overwrite: true);
        _cache = list;
    }

    [System.Runtime.Versioning.SupportedOSPlatform("windows")]
    private static void Protect(string file)
    {
        var security = new FileSecurity();
        security.SetAccessRuleProtection(isProtected: true, preserveInheritance: false);
        foreach (var sid in new[] { WellKnownSidType.LocalSystemSid, WellKnownSidType.BuiltinAdministratorsSid })
            security.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(sid, null),
                FileSystemRights.FullControl, AccessControlType.Allow));
        new FileInfo(file).SetAccessControl(security);
    }

    internal static bool IsValidSid(string? sid)
    {
        if (string.IsNullOrWhiteSpace(sid) || !sid.StartsWith("S-1-", StringComparison.Ordinal)) return false;
        try { _ = new SecurityIdentifier(sid); return true; }
        catch (ArgumentException) { return false; }
    }

    /// <summary>
    /// Appartenance au groupe Administrateurs, que le jeton soit élevé (groupe actif) ou filtré
    /// par l'UAC (groupe « deny-only »). .NET expose ces derniers en revendications DenyOnlySid.
    /// L'appartenance est celle du jeton : un retrait du groupe prend effet à la session suivante.
    /// </summary>
    internal static bool IsAdministratorsMember(IEnumerable<Claim> claims) =>
        claims.Any(c => (c.Type == ClaimTypes.GroupSid || c.Type == ClaimTypes.DenyOnlySid)
                        && c.Value.Equals(AdministratorsSid, StringComparison.OrdinalIgnoreCase));

    /// <summary>Règle d'accès au canal d'administration.</summary>
    internal static bool IsAllowed(bool elevatedAdmin, bool isSystem, bool listed, bool administratorsMember) =>
        elevatedAdmin || isSystem || (listed && administratorsMember);

    /// <summary>Refus d'autorisation : null si permis, sinon la raison affichable.</summary>
    internal static string? AuthorizeRefusal(bool elevatedAdmin, string? sid, string accountName, IEnumerable<string> seatAccounts)
    {
        if (!elevatedAdmin) return "L'autorisation se donne une fois depuis l'application lancée en administrateur";
        if (sid is null || sid == SystemSid || !IsValidSid(sid)) return "Compte système non autorisable";
        if (seatAccounts.Any(a => a.Equals(accountName, StringComparison.OrdinalIgnoreCase)))
            return "Un compte de place ne peut pas administrer Kinseat";
        return null;
    }
}
