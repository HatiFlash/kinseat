using System.Security.Principal;
using System.Text.Json;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Logging.Abstractions;
using MultiSeat.Service.Configuration;
using MultiSeat.Service.Library;
using MultiSeat.Service.SteamShim;
using MultiSeat.Service.Streaming;
using Xunit;

namespace MultiSeat.Tests.Library;

public sealed class GameLaunchManifestTests : IDisposable
{
    private readonly string _root = Path.Combine(Path.GetTempPath(), "game-manifest-" + Guid.NewGuid().ToString("N"));
    public GameLaunchManifestTests() => Directory.CreateDirectory(_root);
    public void Dispose() => Directory.Delete(_root, true);

    // forcePortable : le dossier temporaire des tests peut être sous C:\Windows (compte SYSTEM),
    // que Kinseat ne traite jamais comme un jeu portable.
    private GameLaunchManifestWriter Writer(bool managed = true, bool forcePortable = false) => new(_ => managed,
        _ => (OperatingSystem.IsWindows() ? WindowsIdentity.GetCurrent().User!.Value : "S-1-5-21-1-2-3-1001",
            Path.Combine(_root, "profile")),
        Path.Combine(_root, "games"), Path.Combine(_root, "public"),
        isPortable: forcePortable ? _ => true : null);

    private static GameEntry Game(string exe = @"D:\Games\Sample\play.exe") => new()
    {
        Id = "manual:123", Name = "Sample", Source = "manual", InstallPath = @"D:\Games\Sample",
        ExePath = exe, AssignedTo = ["seat"]
    };

    [Fact]
    public void ClearRemovesOnlyAccountManifestDirectoryAndAcceptsMissingDirectory()
    {
        var games = Path.Combine(_root, "games");
        var writer = new GameLaunchManifestWriter(_ => true, _ => null, games, _root);
        var account = Path.Combine(games, "seat");
        var other = Path.Combine(games, "other");
        Directory.CreateDirectory(account);
        Directory.CreateDirectory(other);
        File.WriteAllText(Path.Combine(account, "manifest.json"), "{}");
        File.WriteAllText(Path.Combine(other, "manifest.json"), "{}");

        writer.Clear("seat");
        Assert.False(Directory.Exists(account));
        Assert.True(File.Exists(Path.Combine(other, "manifest.json")));
        Assert.True(Directory.Exists(games));
        writer.Clear("seat");
    }

    [Theory]
    [InlineData("")]
    [InlineData("../other")]
    [InlineData("seat\\other")]
    [InlineData("seat..backup")]
    public void ClearRejectsInvalidAccountWithoutDeletingAnything(string account)
    {
        var games = Path.Combine(_root, "games");
        var writer = new GameLaunchManifestWriter(_ => true, _ => null, games, _root);
        var victim = Path.Combine(games, "seat");
        Directory.CreateDirectory(victim);
        File.WriteAllText(Path.Combine(victim, "manifest.json"), "{}");

        Assert.Throws<ArgumentException>(() => writer.Clear(account));
        Assert.True(File.Exists(Path.Combine(victim, "manifest.json")));
    }

    [Fact]
    public void PortableAndProtectedDirectories()
    {
        Assert.True(GameLaunchManifestWriter.IsPortable(@"D:\Games\play.exe"));
        Assert.False(GameLaunchManifestWriter.IsPortable(@"C:\Program Files\Game\play.exe"));
        Assert.False(GameLaunchManifestWriter.IsPortable(@"C:\Program Files (x86)\Game\play.exe"));
        Assert.False(GameLaunchManifestWriter.IsPortable(@"C:\Windows\play.exe"));
        Assert.Equal("manual%3A123.json", GameLaunchManifestWriter.FileName("manual:123"));
    }

    [Fact]
    public void ExcludedGamesAndUnmanagedAccountProduceNoManifest()
    {
        var game = Game();
        game.SaveIsolation = "off";
        Assert.Empty(Writer().Sync("seat", [game]));
        game.SaveIsolation = "auto";
        game.AntiCheat = true;
        Assert.Empty(Writer().Sync("seat", [game]));
        game.AntiCheat = false;
        Assert.Empty(Writer(false).Sync("seat", [game]));
        Assert.False(Directory.Exists(Path.Combine(_root, "games")));
    }

    [Fact]
    public void CommandForMoonlightAndShortcutUsesSameManifest()
    {
        var game = Game();
        var manifest = @"C:\ProgramData\MultiSeat\games\seat\manual%3A123.json";
        var launcher = Path.Combine(_root, "Kinseat", "SteamIsolation", "MultiSeat.SteamLauncher.exe");
        var options = new MultiSeatOptions { GameFilesLauncherPath = launcher };
        Assert.Equal(Path.Combine(AppContext.BaseDirectory, "SteamIsolation", "MultiSeat.SteamLauncher.exe"),
            new MultiSeatOptions().GameFilesLauncherPath);
        Assert.Equal(launcher, GameLaunchManifestWriter.LauncherPath(options));
        var writer = new GameLaunchManifestWriter(_ => true, _ => null, _root, _root, _ => true);
        var entries = new Dictionary<string, string> { [game.Id] = manifest };
        var json = ApolloConfigBuilder.AddAssignedGames("{\"apps\":[]}", [game], "seat", manifests: entries,
            launcherPath: GameLaunchManifestWriter.LauncherPath(options), fileExists: writer.FileExistsCheck);
        using var parsed = JsonDocument.Parse(json);
        var expected = $"\"{launcher}\" --game \"{manifest}\" -- \"{game.ExePath}\"";
        Assert.Equal(expected, parsed.RootElement.GetProperty("apps")[0].GetProperty("cmd").GetString());
        var shortcuts = new GameShortcuts(new FakeShortcuts(), _ => null, () => "steam.exe", _root,
            options, NullLogger<GameShortcuts>.Instance, gameManifests: writer);
        var info = shortcuts.Describe(game, entries)!;
        Assert.Equal(launcher, info.Target);
        Assert.Equal($"--game \"{manifest}\" -- \"{game.ExePath}\"", info.Arguments);
        game.LaunchUri = "steam://rungameid/123";
        Assert.Equal("explorer.exe", shortcuts.Describe(game, entries)!.Target);
    }

    [Fact]
    public void ManifestJsonAndSingleImportOnWindows()
    {
        if (!OperatingSystem.IsWindows()) return;
        Directory.CreateDirectory(Path.Combine(_root, "profile"));
        var source = Path.Combine(_root, "public", "Sample");
        Directory.CreateDirectory(source);
        File.WriteAllText(Path.Combine(source, "save.dat"), "host");
        var game = Game();
        game.StartFromHostSave = true;
        var writer = Writer();
        var path = Assert.Single(writer.Sync("seat", [game])).Value;
        Assert.Equal(Path.Combine(_root, "games", "seat", "manual%3A123.json"), path);
        using (var doc = JsonDocument.Parse(File.ReadAllText(path)))
        {
            var json = doc.RootElement;
            Assert.Equal(7, json.EnumerateObject().Count());
            Assert.Equal(game.Id, json.GetProperty("gameId").GetString());
            Assert.Equal("seat", json.GetProperty("account").GetString());
            Assert.Equal(WindowsIdentity.GetCurrent().User!.Value, json.GetProperty("accountSid").GetString());
            Assert.Equal(Path.Combine(_root, "profile"), json.GetProperty("profileDir").GetString());
            Assert.True(json.GetProperty("copyFromHostOnce").GetBoolean());
            Assert.EndsWith("gamefiles-manual%3A123.log", json.GetProperty("logFile").GetString());
            var redirects = json.GetProperty("redirects");
            Assert.Equal(2, redirects.GetArrayLength());
            Assert.Equal("redirect", redirects[0].GetProperty("mode").GetString());
            Assert.Equal(Path.Combine(_root, "public"), redirects[0].GetProperty("from").GetString());
            Assert.Equal("overlay", redirects[1].GetProperty("mode").GetString());
        }
        var save = Path.Combine(_root, "profile", "Documents", "Kinseat", "Sauvegardes", "Sample", "save.dat");
        Assert.Equal("host", File.ReadAllText(save));
        File.WriteAllText(save, "player");
        writer.Sync("seat", [game]);
        Assert.Equal("player", File.ReadAllText(save));
        writer.Sync("seat", []);
        Assert.False(File.Exists(path));
    }

    [Fact]
    public void FindsOnlyShallowNamedSaveDirectoriesAndStopsAtMatchedParent()
    {
        var gameDir = Path.Combine(_root, "longshot");
        Directory.CreateDirectory(Path.Combine(gameDir, "user", "save", "id", "slot_00"));
        Directory.CreateDirectory(Path.Combine(gameDir, "user", "save", "saves"));
        Directory.CreateDirectory(Path.Combine(gameDir, "deep", "one", "two", "save"));
        Directory.CreateDirectory(Path.Combine(gameDir, "savestate"));
        Directory.CreateDirectory(Path.Combine(gameDir, "saved"));
        Directory.CreateDirectory(Path.Combine(gameDir, "SaveGames"));

        var found = GameLaunchManifestWriter.FindSaveDirectories(gameDir);

        Assert.Equal(2, found.Count);
        Assert.Contains(Path.Combine(gameDir, "user", "save"), found);
        Assert.Contains(Path.Combine(gameDir, "SaveGames"), found);
        Assert.DoesNotContain(Path.Combine(gameDir, "user", "save", "saves"), found);
        Assert.DoesNotContain(Path.Combine(gameDir, "deep", "one", "two", "save"), found);
        Assert.DoesNotContain(Path.Combine(gameDir, "savestate"), found);
        Assert.DoesNotContain(Path.Combine(gameDir, "saved"), found);
    }

    [Fact]
    public void ManifestRedirectsPortableSaveFoldersBeforeOverlayUnlessHostSaveEnabled()
    {
        if (!OperatingSystem.IsWindows()) return;
        var profile = Path.Combine(_root, "profile");
        Directory.CreateDirectory(profile);
        var gameDir = Path.Combine(_root, "Longshot");
        var hostSave = Path.Combine(gameDir, "user", "save");
        Directory.CreateDirectory(Path.Combine(hostSave, "id"));
        var game = Game(Path.Combine(gameDir, "longshot.exe"));
        var writer = Writer(forcePortable: true);

        var path = Assert.Single(writer.Sync("seat", [game])).Value;
        using (var doc = JsonDocument.Parse(File.ReadAllText(path)))
        {
            var redirects = doc.RootElement.GetProperty("redirects");
            Assert.Equal(3, redirects.GetArrayLength());
            Assert.Equal(Path.Combine(_root, "public"), redirects[0].GetProperty("from").GetString());
            Assert.Equal(hostSave, redirects[1].GetProperty("from").GetString());
            Assert.Equal("redirect", redirects[1].GetProperty("mode").GetString());
            Assert.Equal(Path.Combine(profile, "AppData", "Local", "Kinseat", "Jeux", "manual%3A123.sauvegardes", "user", "save"),
                redirects[1].GetProperty("to").GetString());
            Assert.Equal("overlay", redirects[2].GetProperty("mode").GetString());
        }

        game.StartFromHostSave = true;
        writer.Sync("seat", [game]);
        using var hostDoc = JsonDocument.Parse(File.ReadAllText(path));
        var hostRedirects = hostDoc.RootElement.GetProperty("redirects");
        Assert.Equal(2, hostRedirects.GetArrayLength());
        Assert.Equal("overlay", hostRedirects[1].GetProperty("mode").GetString());
    }

    [Fact]
    public void CopyOnlyNamedFolderOnceWithoutOverwritingPlayerSave()
    {
        var source = Path.Combine(_root, "public", "Sample");
        var destination = Path.Combine(_root, "save");
        Directory.CreateDirectory(source);
        File.WriteAllText(Path.Combine(source, "save.dat"), "host");
        Directory.CreateDirectory(Path.Combine(_root, "public", "Other"));
        File.WriteAllText(Path.Combine(_root, "public", "Other", "save.dat"), "foreign");
        GameLaunchManifestWriter.CopyIdentifiedHostSave(Path.Combine(_root, "public"), "Sample", destination);
        var copied = Path.Combine(destination, "Sample", "save.dat");
        Assert.Equal("host", File.ReadAllText(copied));
        File.WriteAllText(copied, "player");
        GameLaunchManifestWriter.CopyIdentifiedHostSave(Path.Combine(_root, "public"), "Sample", destination);
        Assert.Equal("player", File.ReadAllText(copied));
        Assert.False(Directory.Exists(Path.Combine(destination, "Other")));
    }

    [Theory]
    [InlineData(false, false)]
    [InlineData(true, false)]
    public void MissingLauncherOrGameFilesDllSkipsGameAndWarnsOnce(bool launcherPresent, bool dllPresent)
    {
        var launcher = Path.Combine(_root, "SteamIsolation", "MultiSeat.SteamLauncher.exe");
        var dll = Path.Combine(_root, "SteamIsolation", "MultiSeat.GameFiles.dll");
        var options = new MultiSeatOptions { GameFilesLauncherPath = launcher };
        var log = new WarningLogger();
        var writer = new GameLaunchManifestWriter(_ => true, _ => null, _root, _root,
            path => path == launcher ? launcherPresent : path == dll && dllPresent, log);
        var game = Game();
        var entries = new Dictionary<string, string> { [game.Id] = "manifest.json" };
        var shortcuts = new GameShortcuts(new FakeShortcuts(), _ => null, () => "steam.exe", _root,
            options, NullLogger<GameShortcuts>.Instance, gameManifests: writer);

        Assert.Null(shortcuts.Describe(game, entries)); // Fail closed: no direct shortcut.
        Assert.False(writer.LauncherAvailable(options)); // Shared warning across Apollo and shortcuts.
        var json = ApolloConfigBuilder.AddAssignedGames("{\"apps\":[]}", [game], "seat", manifests: entries,
            launcherPath: launcher, fileExists: writer.FileExistsCheck);
        using var parsed = JsonDocument.Parse(json);
        Assert.Empty(parsed.RootElement.GetProperty("apps").EnumerateArray()); // Fail closed: no direct launch.
        Assert.Equal(1, log.Warnings);
    }

    [Fact]
    public void IsolatableGameWithoutManifestIsSkippedAndReported()
    {
        var game = Game();
        var skipped = new List<string>();
        var json = ApolloConfigBuilder.AddAssignedGames("{\"apps\":[]}", [game], "seat", skipped: skipped.Add);
        Assert.Empty(JsonDocument.Parse(json).RootElement.GetProperty("apps").EnumerateArray());
        Assert.Equal([game.Name], skipped);
    }

    [Fact]
    public void IsolatableGameUsesLauncherWhenManifestAndLauncherExist()
    {
        var game = Game();
        var launcher = Path.Combine(_root, "SteamIsolation", "MultiSeat.SteamLauncher.exe");
        var manifest = Path.Combine(_root, "manifest.json");
        var entries = new Dictionary<string, string> { [game.Id] = manifest };
        var json = ApolloConfigBuilder.AddAssignedGames("{\"apps\":[]}", [game], "seat", manifests: entries,
            launcherPath: launcher, fileExists: _ => true);
        var command = Assert.Single(JsonDocument.Parse(json).RootElement.GetProperty("apps").EnumerateArray())
            .GetProperty("cmd").GetString();
        Assert.Contains("--game", command);
        Assert.Contains(manifest, command);
    }

    private sealed class WarningLogger : ILogger<GameLaunchManifestWriter>
    {
        public int Warnings { get; private set; }
        public IDisposable? BeginScope<TState>(TState state) where TState : notnull => null;
        public bool IsEnabled(LogLevel level) => true;
        public void Log<TState>(LogLevel level, EventId eventId, TState state, Exception? exception,
            Func<TState, Exception?, string> formatter)
        {
            if (level == LogLevel.Warning) Warnings++;
        }
    }

    private sealed class FakeShortcuts : IShortcutStore
    {
        public ShortcutInfo? Read(string path) => null;
        public void Write(string path, ShortcutInfo info) { }
    }
}
