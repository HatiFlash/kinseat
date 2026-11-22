using System.Text.Json;
using Microsoft.Extensions.Logging.Abstractions;
using MultiSeat.Service.Ipc;
using MultiSeat.Service.Library;
using MultiSeat.Service.Streaming;
using Xunit;

namespace MultiSeat.Tests.Library;

public sealed class LibraryTests
{
    private static string Temp() { var path = Path.Combine(Path.GetTempPath(), "library-test-" + Guid.NewGuid().ToString("N")); Directory.CreateDirectory(path); return path; }
    private static GameEntry Game(string id = "steam:42") => new()
    { Id = id, Name = "Jeu", Source = "steam", InstallPath = @"D:\SteamLibrary\steamapps\common\Jeu", LaunchUri = "steam://rungameid/42" };

    [Theory]
    [InlineData(@"C:\")]
    [InlineData(@"C:\Users")]
    [InlineData(@"C:\Users\alice")]
    [InlineData(@"C:\Users\alice\Desktop")]
    [InlineData(@"C:\Users\alice\Documents")]
    [InlineData(@"C:\Users\alice\AppData")]
    [InlineData(@"C:\Users\alice\OneDrive")]
    [InlineData(@"C:\Users\alice\Downloads")]
    [InlineData(@"C:\Users\alice\CustomFolder")]
    [InlineData(@"C:\Windows")]
    [InlineData(@"C:\Windows\System32")]
    [InlineData(@"C:\Program Files")]
    [InlineData(@"C:\Program Files (x86)")]
    [InlineData(@"C:\ProgramData")]
    [InlineData(@"\\?\C:\Users\alice\Desktop\")]
    [InlineData(@"c:\uSeRs\Alice\appdata/")]
    [InlineData(@"\\server\share\game")]
    [InlineData(@"C:\Jeux\..\Windows")]
    public void Access_RejectsProtectedDirectories(string directory) => Assert.False(LibraryAccess.AllowedDirectory(directory));

    [Theory]
    [InlineData(@"C:\Users\alice\Desktop\jeux\hookshot")]
    [InlineData(@"D:\Games\X")]
    [InlineData(@"C:\Jeux\X")]
    [InlineData(@"\\?\C:\Jeux\X\")]
    [InlineData(@"d:\games\x/")]
    public void Access_AllowsSpecificGameDirectories(string directory) => Assert.True(LibraryAccess.AllowedDirectory(directory));

    [Fact]
    public void Access_ResponsesArePerAssignedAccountAndNotPersisted()
    {
        var root = Temp();
        try
        {
            var path = Path.Combine(root, "library.json");
            var store = new LibraryStore(path);
            store.Merge([Game()]);
            var assigned = store.Assign("steam:42", "ks-a", true).Games.Single();
            Assert.Equal("ok", assigned.Access!["ks-a"]);
            var json = JsonSerializer.Serialize(assigned, new JsonSerializerOptions(JsonSerializerDefaults.Web));
            using var response = JsonDocument.Parse(json);
            Assert.Equal("ok", response.RootElement.GetProperty("access").GetProperty("ks-a").GetString());
            Assert.DoesNotContain("\"access\"", File.ReadAllText(path), StringComparison.OrdinalIgnoreCase);
            var missing = Game(); missing.ExePath = Path.Combine(root, "absent.exe");
            store.Merge([missing]);
            Assert.Equal("missing", store.Get().Games.Single().Access!["ks-a"]);
            Assert.Empty(store.Assign("steam:42", "ks-a", false).Games.Single().Access!);
            Assert.Empty(store.Add("Test", MakeExe(root)).Games.Single(g => g.Source == "manual").Access!);
        }
        finally { Directory.Delete(root, true); }
    }

    private static string MakeExe(string root)
    {
        var path = Path.Combine(root, "manual.exe");
        File.WriteAllText(path, "test");
        return path;
    }

    [Fact]
    public void Grants_InventoryRemovesOnlyMatchingRecords_WhenGameFolderIsGone()
    {
        var root = Temp();
        try
        {
            var path = Path.Combine(root, "library-grants.json");
            var records = new[]
            {
                new { gameId = "a", account = "ks-a", directory = @"D:\Gone\Game", sid = "S-1-5-21-1-2-3-1001", ownsRule = true },
                new { gameId = "b", account = "ks-a", directory = @"D:\Gone\Game", sid = "S-1-5-21-1-2-3-1001", ownsRule = false },
                new { gameId = "c", account = "ks-b", directory = @"D:\Gone\Game", sid = "S-1-5-21-1-2-3-1002", ownsRule = true }
            };
            File.WriteAllText(path, JsonSerializer.Serialize(records));
            var inventory = new LibraryGrants(path);
            inventory.Remove("a", "KS-A");
            using var afterGame = JsonDocument.Parse(File.ReadAllText(path));
            Assert.Equal("b", afterGame.RootElement[0].GetProperty("gameId").GetString());
            Assert.True(afterGame.RootElement[0].GetProperty("ownsRule").GetBoolean());
            inventory.Remove(null, "ks-b");
            using var afterAccount = JsonDocument.Parse(File.ReadAllText(path));
            Assert.Single(afterAccount.RootElement.EnumerateArray());
            inventory.Remove("b", "ks-a");
            Assert.Equal("[]", File.ReadAllText(path));
        }
        finally { Directory.Delete(root, true); }
    }

    [Fact]
    public async Task GrantAccess_RequiresAssignedAccountAndRejectsProtectedFolder()
    {
        var root = Temp();
        try
        {
            var store = new LibraryStore(Path.Combine(root, "library.json"));
            store.Merge([new GameEntry { Id = "x", Name = "X", Source = "steam", InstallPath = @"C:\Users\alice\Desktop", LaunchUri = "steam://x" }]);
            Assert.Equal("invalid_params", Assert.Throws<AdminFault>(() => store.GrantAccess("x", "ks-a")).Code);
            store.Assign("x", "ks-a", true);
            var handler = new LibraryIpcHandler(store, new GameScannerRunner([], NullLogger<GameScannerRunner>.Instance));
            Assert.Contains("library.grantAccess", LibraryIpcHandler.Methods);
            var args = JsonSerializer.SerializeToElement(new { gameId = "x", accountName = "ks-a" });
            Assert.Equal("invalid_params", (await Assert.ThrowsAsync<AdminFault>(() => handler.HandleAsync("library.grantAccess", args, default))).Code);
        }
        finally { Directory.Delete(root, true); }
    }

    [Fact]
    public void RevokeAccount_RemovesTheAccountFromEveryGameOnly()
    {
        var root = Temp();
        try
        {
            var store = new LibraryStore(Path.Combine(root, "library.json"));
            store.Merge(new[] { Game("steam:1"), Game("steam:2") });
            store.Assign("steam:1", "ks-a", true); store.Assign("steam:2", "KS-A", true); store.Assign("steam:2", "ks-b", true);
            Assert.Equal(2, store.RevokeAccount("ks-a"));
            var reloaded = new LibraryStore(Path.Combine(root, "library.json")).Get().Games;
            Assert.Empty(reloaded.Single(g => g.Id == "steam:1").AssignedTo);
            Assert.Equal(new[] { "ks-b" }, reloaded.Single(g => g.Id == "steam:2").AssignedTo);
            Assert.Equal(0, store.RevokeAccount("ks-a"));
        }
        finally { Directory.Delete(root, true); }
    }

    [Fact]
    public void Steam_ParsesNestedLibrariesEscapesAndManifest()
    {
        var root = Temp();
        try
        {
            var steamapps = Path.Combine(root, "steamapps"); Directory.CreateDirectory(steamapps);
            // Path.Combine(root, "D:\\…") renverrait un chemin absolu sur D: (absent des VM) : bibliothèque
            // secondaire simulée sous root, le chemin échappé du VDF reste testé ci-dessous.
            var other = Path.Combine(root, "SteamLibrary");
            Directory.CreateDirectory(Path.Combine(other, "steamapps"));
            var vdfPath = other.Replace("\\", "\\\\");
            File.WriteAllText(Path.Combine(steamapps, "libraryfolders.vdf"),
                "\"libraryfolders\" { \"0\" { \"path\" \"" + vdfPath + "\" \"apps\" { \"42\" \"1\" } } }");
            File.WriteAllText(Path.Combine(other, "steamapps", "appmanifest_42.acf"),
                "\"AppState\" { \"appid\" \"42\" \"name\" \"Game \\\"42\\\"\" \"installdir\" \"Jeu\" \"SizeOnDisk\" \"128\" }");
            File.WriteAllText(Path.Combine(other, "steamapps", "appmanifest_228980.acf"),
                "\"AppState\" { \"appid\" \"228980\" \"name\" \"Steamworks Common Redistributables\" \"installdir\" \"Tools\" }");
            var game = Assert.Single(new SteamGameScanner(root).Scan());
            Assert.Equal("steam:42", game.Id);
            Assert.Equal("Game \"42\"", game.Name);
            Assert.Equal(128, game.SizeBytes);
            Assert.Equal("steam://rungameid/42", game.LaunchUri);
            Assert.Equal(Path.Combine(other, "steamapps", "common", "Jeu"), game.InstallPath);
            var parsed = ValveKeyValues.Parse("\"path\" \"D:\\\\SteamLibrary\" ");
            Assert.Equal(@"D:\SteamLibrary", ValveKeyValues.Text(parsed, "path"));
        }
        finally { Directory.Delete(root, true); }
    }

    [Fact]
    public void Epic_SkipsIncompleteAndReadsManifest()
    {
        var root = Temp();
        try
        {
            File.WriteAllText(Path.Combine(root, "game.item"), """{"DisplayName":"Hades","InstallLocation":"D:\\Games\\Hades","AppName":"HadesApp","LaunchExecutable":"Hades.exe"}""");
            File.WriteAllText(Path.Combine(root, "partial.item"), """{"DisplayName":"Missing","InstallLocation":"X","AppName":"Partial","bIsIncompleteInstall":true}""");
            var game = Assert.Single(new EpicGameScanner(root).Scan());
            Assert.Equal("epic:HadesApp", game.Id);
            Assert.Equal(@"D:\Games\Hades", game.InstallPath);
            Assert.Equal("com.epicgames.launcher://apps/HadesApp?action=launch&silent=true", game.LaunchUri);
        }
        finally { Directory.Delete(root, true); }
    }

    [Fact]
    public void Merge_PreservesAssignmentsAndManualEntries()
    {
        var root = Temp();
        try
        {
            var store = new LibraryStore(Path.Combine(root, "library.json"));
            store.Merge([Game(), Game("steam:old")]);
            store.Assign("steam:42", "seat1", true);
            var exe = Path.Combine(root, "manual.exe"); File.WriteAllText(exe, "test");
            store.Add("Manual", exe);
            var snapshot = store.Merge([Game()]);
            Assert.Equal(2, snapshot.Games.Count);
            Assert.Equal("seat1", Assert.Single(snapshot.Games.Single(g => g.Id == "steam:42").AssignedTo));
            Assert.Contains(snapshot.Games, g => g.Source == "manual");
            Assert.DoesNotContain(snapshot.Games, g => g.Id == "steam:old");
            Assert.Equal("seat1", Assert.Single(new LibraryStore(Path.Combine(root, "library.json")).Get().Games.Single(g => g.Id == "steam:42").AssignedTo));
        }
        finally { Directory.Delete(root, true); }
    }

    [Fact]
    public void ManualId_IsStableForCaseVariations()
    {
        var path = Path.Combine(Temp(), "Game.exe");
        try { Assert.Equal(LibraryStore.ManualId(path), LibraryStore.ManualId(path.ToUpperInvariant())); }
        finally { Directory.Delete(Path.GetDirectoryName(path)!, true); }
    }

    [Fact]
    public async Task Handler_RejectsMissingExeAndAutomaticRemoval()
    {
        var root = Temp();
        try
        {
            var store = new LibraryStore(Path.Combine(root, "library.json")); store.Merge([Game()]);
            var handler = new LibraryIpcHandler(store, new GameScannerRunner([], NullLogger<GameScannerRunner>.Instance));
            var add = JsonSerializer.SerializeToElement(new { name = "Missing", exePath = Path.Combine(root, "absent.exe") });
            Assert.Equal("invalid_params", (await Assert.ThrowsAsync<AdminFault>(() => handler.HandleAsync("library.add", add, default))).Code);
            var remove = JsonSerializer.SerializeToElement(new { id = "steam:42" });
            Assert.Equal("invalid_params", (await Assert.ThrowsAsync<AdminFault>(() => handler.HandleAsync("library.remove", remove, default))).Code);
        }
        finally { Directory.Delete(root, true); }
    }

    [Fact]
    public void ApolloApps_AddsAssignedGameBesideDesktop()
    {
        var game = Game(); game.AssignedTo.Add("seat1");
        game.LaunchUri = "steam://rungameid/42";
        var json = ApolloConfigBuilder.AddAssignedGames("""{"apps":[{"name":"Desktop","image-path":"desktop.png"}]}""", [game], "seat1");
        using var document = JsonDocument.Parse(json);
        var apps = document.RootElement.GetProperty("apps");
        Assert.Equal(2, apps.GetArrayLength());
        Assert.Equal("Desktop", apps[0].GetProperty("name").GetString());
        Assert.Equal("Jeu", apps[1].GetProperty("name").GetString());
        Assert.Equal("cmd /c start \"\" \"steam://rungameid/42\"", apps[1].GetProperty("cmd").GetString());
        Assert.Single(JsonDocument.Parse(ApolloConfigBuilder.AddAssignedGames("""{"apps":[{"name":"Desktop"}]}""", [game], "other")).RootElement.GetProperty("apps").EnumerateArray());
    }

    [Fact]
    public void LegacyJson_UsesSafeDefaultsAndResponseFieldsAreNotPersisted()
    {
        var root = Temp();
        try
        {
            var path = Path.Combine(root, "library.json");
            File.WriteAllText(path, """{"games":[{"id":"manual:old","name":"Old","source":"manual","installPath":"missing","exePath":"missing.exe","assignedTo":[]}]}""");
            var store = new LibraryStore(path);
            var game = Assert.Single(store.Get().Games);
            Assert.Equal("auto", game.SaveIsolation);
            Assert.False(game.StartFromHostSave);
            Assert.False(game.AntiCheat);
            Assert.Equal("redirected", game.SaveLocation);
            Assert.Equal("manual", game.Store);
            store.SetOptions(game.Id, "off", true);
            var persisted = File.ReadAllText(path);
            Assert.Contains("\"saveIsolation\": \"off\"", persisted);
            Assert.Contains("\"startFromHostSave\": true", persisted);
            Assert.DoesNotContain("\"saveLocation\"", persisted);
            Assert.DoesNotContain("\"store\"", persisted);
            var reloaded = Assert.Single(new LibraryStore(path).Get().Games);
            Assert.Equal("off", reloaded.SaveIsolation);
            Assert.True(reloaded.StartFromHostSave);
            Assert.Equal("unknown", reloaded.SaveLocation);
        }
        finally { Directory.Delete(root, true); }
    }

    [Fact]
    public void AntiCheat_IsCaseInsensitiveAndStopsAtDepthThree()
    {
        var root = Temp();
        try
        {
            var third = Path.Combine(root, "one", "two");
            Directory.CreateDirectory(third);
            var fourth = Path.Combine(third, "three");
            Directory.CreateDirectory(fourth);
            File.WriteAllText(Path.Combine(fourth, "EAAntiCheat.exe"), "");
            Assert.False(AntiCheatDetector.Detect(root));
            File.WriteAllText(Path.Combine(third, "eAsYaNtIcHeAt_x64.dll"), "");
            Assert.True(AntiCheatDetector.Detect(root));
            var store = new LibraryStore(Path.Combine(root, "library.json"));
            var game = store.Add("Game", MakeExe(root)).Games.Single();
            Assert.True(game.AntiCheat);
            Assert.Equal("unknown", game.SaveLocation);
        }
        finally { Directory.Delete(root, true); }
    }

    [Fact]
    public async Task SetOptions_ValidatesAndSurvivesScan()
    {
        var root = Temp();
        try
        {
            var path = Path.Combine(root, "library.json");
            var store = new LibraryStore(path);
            store.Merge([Game()]);
            var handler = new LibraryIpcHandler(store, new GameScannerRunner([], NullLogger<GameScannerRunner>.Instance));
            Assert.Contains("library.setOptions", LibraryIpcHandler.Methods);
            async Task<string> Fault(object args) => (await Assert.ThrowsAsync<AdminFault>(() =>
                handler.HandleAsync("library.setOptions", JsonSerializer.SerializeToElement(args), default))).Code;
            Assert.Equal("not_found", await Fault(new { gameId = "missing", saveIsolation = "off" }));
            Assert.Equal("invalid_params", await Fault(new { gameId = "steam:42", saveIsolation = "invalid" }));
            Assert.Equal("invalid_params", await Fault(new { gameId = "steam:42", startFromHostSave = "true" }));
            Assert.Equal("invalid_params", await Fault(new { gameId = "steam:42", extra = true }));
            var updated = (LibrarySnapshot)await handler.HandleAsync("library.setOptions",
                JsonSerializer.SerializeToElement(new { gameId = "steam:42", saveIsolation = "off", startFromHostSave = true }), default);
            Assert.Equal("cloud:Steam", Assert.Single(updated.Games).SaveLocation);
            store.Merge([Game()]);
            var reloaded = Assert.Single(new LibraryStore(path).Get().Games);
            Assert.Equal("off", reloaded.SaveIsolation);
            Assert.True(reloaded.StartFromHostSave);
        }
        finally { Directory.Delete(root, true); }
    }
}
