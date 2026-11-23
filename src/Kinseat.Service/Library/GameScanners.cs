using System.Globalization;
using System.Text;
using System.Text.Json;
using Microsoft.Extensions.Logging;
using Microsoft.Win32;

namespace MultiSeat.Service.Library;

public interface IGameScanner
{
    string Source { get; }
    IEnumerable<GameEntry> Scan();
}

// KeyValues is a token stream, not a line format: quoted strings may contain braces and escapes.
internal static class ValveKeyValues
{
    internal sealed class Node : Dictionary<string, object> { public Node() : base(StringComparer.OrdinalIgnoreCase) { } }

    public static Node Parse(string text)
    {
        var index = 0;
        string? Next()
        {
            while (index < text.Length)
            {
                if (char.IsWhiteSpace(text[index])) { index++; continue; }
                if (text[index] == '/' && index + 1 < text.Length && text[index + 1] == '/')
                { while (index < text.Length && text[index] != '\n') index++; continue; }
                break;
            }
            if (index == text.Length) return null;
            if (text[index] is '{' or '}') return text[index++].ToString();
            if (text[index] != '"') throw new FormatException("Chaîne VDF attendue");
            index++;
            var value = new StringBuilder();
            while (index < text.Length)
            {
                var ch = text[index++];
                if (ch == '"') return value.ToString();
                if (ch == '\\' && index < text.Length && text[index] is '\\' or '"') ch = text[index++];
                value.Append(ch);
            }
            throw new FormatException("Chaîne VDF non terminée");
        }
        Node Read(bool nested)
        {
            var node = new Node();
            while (true)
            {
                var key = Next();
                if (key is null) { if (nested) throw new FormatException("Accolade VDF manquante"); return node; }
                if (key == "}") { if (!nested) throw new FormatException("Accolade VDF inattendue"); return node; }
                if (key == "{") throw new FormatException("Clé VDF manquante");
                var value = Next() ?? throw new FormatException("Valeur VDF manquante");
                if (value == "}") throw new FormatException("Valeur VDF manquante");
                node[key] = value == "{" ? Read(true) : value;
            }
        }
        return Read(false);
    }

    public static Node? Section(Node node, string key) => node.TryGetValue(key, out var value) ? value as Node : null;
    public static string? Text(Node node, string key) => node.TryGetValue(key, out var value) ? value as string : null;
}

public sealed class SteamGameScanner(string? installRoot = null) : IGameScanner
{
    public string Source => "steam";
    public IEnumerable<GameEntry> Scan()
    {
        var root = installRoot ?? ReadInstallPath();
        if (string.IsNullOrWhiteSpace(root) || !Directory.Exists(root)) return [];
        var libraries = new HashSet<string>(StringComparer.OrdinalIgnoreCase) { root };
        var folders = Path.Combine(root, "steamapps", "libraryfolders.vdf");
        if (File.Exists(folders))
        {
            var section = ValveKeyValues.Section(ValveKeyValues.Parse(File.ReadAllText(folders)), "libraryfolders");
            if (section is not null)
                foreach (var (_, value) in section)
                {
                    var path = value is ValveKeyValues.Node n ? ValveKeyValues.Text(n, "path") : value as string;
                    if (!string.IsNullOrWhiteSpace(path)) libraries.Add(path);
                }
        }
        var games = new List<GameEntry>();
        foreach (var library in libraries)
        {
            var apps = Path.Combine(library, "steamapps");
            if (!Directory.Exists(apps)) continue;
            foreach (var manifest in Directory.EnumerateFiles(apps, "appmanifest_*.acf"))
            {
                try
                {
                    var data = ValveKeyValues.Section(ValveKeyValues.Parse(File.ReadAllText(manifest)), "AppState");
                    if (data is null) continue;
                    var id = ValveKeyValues.Text(data, "appid");
                    var name = ValveKeyValues.Text(data, "name");
                    var dir = ValveKeyValues.Text(data, "installdir");
                    if (string.IsNullOrWhiteSpace(id) || !id.All(char.IsAsciiDigit) || string.IsNullOrWhiteSpace(name) || string.IsNullOrWhiteSpace(dir)
                        || id == "228980" || name.Contains("Proton", StringComparison.OrdinalIgnoreCase)
                        || name.Contains("Steam Linux Runtime", StringComparison.OrdinalIgnoreCase)) continue;
                    long? size = long.TryParse(ValveKeyValues.Text(data, "SizeOnDisk"), NumberStyles.Integer, CultureInfo.InvariantCulture, out var bytes) ? bytes : null;
                    games.Add(new GameEntry { Id = "steam:" + id, Name = name, Source = Source,
                        InstallPath = Path.Combine(apps, "common", dir), LaunchUri = "steam://rungameid/" + id, SizeBytes = size });
                }
                catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or FormatException)
                { /* A single broken manifest cannot hide the other games. */ }
            }
        }
        return games.DistinctBy(g => g.Id, StringComparer.OrdinalIgnoreCase).ToArray();
    }

    private static string? ReadInstallPath()
    {
        using var machine = RegistryKey.OpenBaseKey(RegistryHive.LocalMachine, RegistryView.Registry64);
        using var key = machine.OpenSubKey(@"SOFTWARE\WOW6432Node\Valve\Steam");
        return key?.GetValue("InstallPath") as string;
    }
}

public sealed class EpicGameScanner(string? manifestsRoot = null) : IGameScanner
{
    public string Source => "epic";
    public IEnumerable<GameEntry> Scan()
    {
        var root = manifestsRoot ?? @"C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests";
        if (!Directory.Exists(root)) return [];
        var games = new List<GameEntry>();
        foreach (var file in Directory.EnumerateFiles(root, "*.item"))
        {
            try
            {
                using var doc = JsonDocument.Parse(File.ReadAllText(file));
                var item = doc.RootElement;
                string? Get(string key) => item.TryGetProperty(key, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() : null;
                if (item.TryGetProperty("bIsIncompleteInstall", out var incomplete) && incomplete.ValueKind == JsonValueKind.True) continue;
                var app = Get("AppName"); var name = Get("DisplayName"); var location = Get("InstallLocation");
                if (string.IsNullOrWhiteSpace(app) || string.IsNullOrWhiteSpace(name) || string.IsNullOrWhiteSpace(location)) continue;
                var executable = Get("LaunchExecutable");
                games.Add(new GameEntry { Id = "epic:" + app, Name = name, Source = Source, InstallPath = location,
                    ExePath = string.IsNullOrWhiteSpace(executable) ? null : Path.Combine(location, executable),
                    LaunchUri = "com.epicgames.launcher://apps/" + Uri.EscapeDataString(app) + "?action=launch&silent=true" });
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or JsonException or ArgumentException)
            { /* Ignore an individual corrupt manifest. */ }
        }
        return games.DistinctBy(g => g.Id, StringComparer.OrdinalIgnoreCase).ToArray();
    }
}

public abstract class RegistryGameScanner(string source, string registryPath) : IGameScanner
{
    public string Source => source;
    public IEnumerable<GameEntry> Scan()
    {
        using var machine = RegistryKey.OpenBaseKey(RegistryHive.LocalMachine, RegistryView.Registry64);
        using var root = machine.OpenSubKey(registryPath);
        if (root is null) return [];
        var entries = new List<GameEntry>();
        foreach (var name in root.GetSubKeyNames())
        {
            using var key = root.OpenSubKey(name);
            if (key is null) continue;
            var entry = Create(name, key);
            if (entry is not null) entries.Add(entry);
        }
        return entries;
    }
    protected abstract GameEntry? Create(string id, RegistryKey key);
}

public sealed class GogGameScanner(string? registryPath = null) : RegistryGameScanner("gog", registryPath ?? @"SOFTWARE\WOW6432Node\GOG.com\Games")
{
    protected override GameEntry? Create(string id, RegistryKey key)
    {
        var path = key.GetValue("PATH") as string;
        if (string.IsNullOrWhiteSpace(path)) return null;
        var exe = key.GetValue("EXE") as string;
        var gameId = key.GetValue("gameID")?.ToString();
        return new GameEntry { Id = "gog:" + (string.IsNullOrWhiteSpace(gameId) ? id : gameId),
            Name = key.GetValue("GAMENAME") as string ?? id, Source = Source, InstallPath = path,
            ExePath = string.IsNullOrWhiteSpace(exe) ? null : Path.IsPathRooted(exe) ? exe : Path.Combine(path, exe) };
    }
}

public sealed class EaGameScanner(string? registryPath = null) : RegistryGameScanner("ea", registryPath ?? @"SOFTWARE\WOW6432Node\Electronic Arts")
{
    protected override GameEntry? Create(string id, RegistryKey key)
    {
        var path = key.GetValue("Install Dir") as string;
        return string.IsNullOrWhiteSpace(path) ? null : new GameEntry
        { Id = "ea:" + id, Name = id, Source = Source, InstallPath = path };
    }
}

public sealed class UbisoftGameScanner(string? registryPath = null) : RegistryGameScanner("ubisoft", registryPath ?? @"SOFTWARE\WOW6432Node\Ubisoft\Launcher\Installs")
{
    protected override GameEntry? Create(string id, RegistryKey key)
    {
        var path = key.GetValue("InstallDir") as string;
        return string.IsNullOrWhiteSpace(path) ? null : new GameEntry
        { Id = "ubisoft:" + id, Name = id, Source = Source, InstallPath = path,
            LaunchUri = "uplay://launch/" + id + "/0" };
    }
}

public sealed class GameScannerRunner(IEnumerable<IGameScanner> scanners, ILogger<GameScannerRunner> logger)
{
    // Failed sources must not purge their previous entries from the persisted catalogue.
    public (List<GameEntry> Games, HashSet<string> Failed) Scan()
    {
        var games = new List<GameEntry>();
        var failed = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var scanner in scanners)
            try { games.AddRange(scanner.Scan()); }
            catch (Exception ex) { logger.LogWarning(ex, "Scan de {Source} impossible", scanner.Source); failed.Add(scanner.Source); }
        return (games, failed);
    }
}
