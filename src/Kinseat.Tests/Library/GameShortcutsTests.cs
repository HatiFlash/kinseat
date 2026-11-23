using System.Reflection.PortableExecutable;
using Microsoft.Extensions.Logging.Abstractions;
using MultiSeat.Service.Configuration;
using MultiSeat.Service.Library;
using MultiSeat.Service.SteamShim;
using Xunit;

namespace MultiSeat.Tests.Library;

public sealed class GameShortcutsTests
{
    private static GameEntry Game(string id, string name, params string[] accounts) => new()
    {
        Id = id, Name = name, Source = "manual", InstallPath = @"C:\Games", ExePath = @"C:\Games\play.exe",
        AssignedTo = [.. accounts]
    };

    [Fact]
    public void Plan_AddsAndRemovesOnlyAssignedGames()
    {
        var desk = @"C:\Users\seat\Desktop";
        var old = new GameShortcuts.OwnedLink(Path.Combine(desk, "Old.lnk"), @"C:\Old.exe", "old");
        var plan = GameShortcuts.Plan(desk, [Game("1", "One", "seat"), Game("2", "Two", "other")],
            "seat", [old], g => new ShortcutInfo(g.ExePath!, "", @"C:\Games", g.ExePath!, g.Name));
        Assert.Equal(old, Assert.Single(plan.Remove));
        Assert.Equal("1", Assert.Single(plan.Add).Link.GameId);
    }

    [Fact]
    public void SafeNamesAndCollisions()
    {
        Assert.Equal("_CON", GameShortcuts.SafeName("CON"));
        Assert.Equal("A_B_", GameShortcuts.SafeName("A:B?"));
        var plan = GameShortcuts.Plan(@"C:\Users\seat\Desktop",
            [Game("1", "A:B", "seat"), Game("2", "A?B", "seat")], "seat", [],
            g => new ShortcutInfo(g.ExePath!, "", "", "", g.Name));
        Assert.Equal(2, plan.Add.Count);
        Assert.NotEqual(plan.Add[0].Link.Path, plan.Add[1].Link.Path);
    }

    [Fact]
    public void ForeignDesktopFileIsNeverOverwrittenOrDeleted()
    {
        var root = Path.Combine(Path.GetTempPath(), "shortcuts-" + Guid.NewGuid().ToString("N"));
        var desktop = Path.Combine(root, "Desktop");
        Directory.CreateDirectory(desktop);
        var foreign = Path.Combine(desktop, "Owned.lnk");
        File.WriteAllText(foreign, "player");
        try
        {
            var links = new FakeStore();
            var shortcuts = new GameShortcuts(links, _ => root, () => "steam.exe", Path.Combine(root, "inventory"),
                new MultiSeatOptions(), NullLogger<GameShortcuts>.Instance);
            shortcuts.Sync("seat", [Game("1", "Owned", "seat")]);
            Assert.Equal("player", File.ReadAllText(foreign));
            shortcuts.Clear("seat");
            Assert.Equal("player", File.ReadAllText(foreign));
        }
        finally { Directory.Delete(root, true); }
    }

    [Fact]
    public void ExeLinkHasWorkingDirectoryAndIcon()
    {
        var shortcuts = new GameShortcuts(new FakeStore(), _ => null, () => "steam.exe", "inventory",
            new MultiSeatOptions(), NullLogger<GameShortcuts>.Instance);
        // Isolation explicitly off: a direct link is the expected shortcut.
        var game = Game("1", "Play", "seat"); game.SaveIsolation = "off";
        var info = shortcuts.Describe(game)!;
        Assert.Equal(@"C:\Games\play.exe", info.Target);
        Assert.Equal(@"C:\Games\play.exe", info.IconLocation);
        Assert.Equal(Path.GetDirectoryName(info.Target), info.WorkingDirectory);
    }

    [Fact]
    public void IsolatableExeWithoutManifestGetsNoDirectShortcut()
    {
        var shortcuts = new GameShortcuts(new FakeStore(), _ => null, () => "steam.exe", "inventory",
            new MultiSeatOptions(), NullLogger<GameShortcuts>.Instance);
        Assert.Null(shortcuts.Describe(Game("1", "Play", "seat")));
    }

    [Fact]
    public void SteamShortcutUsesSameWrappedCommandAsMoonlight()
    {
        var root = Path.Combine(Path.GetTempPath(), "steam-shortcut-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(root);
        try
        {
            string Fixture(string name)
            {
                var path = Path.Combine(root, name);
                var bytes = new byte[1024];
                using (var writer = new BinaryWriter(new MemoryStream(bytes)))
                {
                    writer.Write((ushort)0x5a4d);
                    writer.BaseStream.Position = 0x3c;
                    writer.Write(0x80);
                    writer.BaseStream.Position = 0x80;
                    writer.Write(0x00004550);
                    writer.Write((ushort)Machine.Amd64);
                    writer.BaseStream.Position = 0x94;
                    writer.Write((ushort)240);
                    writer.Write((ushort)0x0002);
                    writer.Write((ushort)0x20b);
                }
                File.WriteAllBytes(path, bytes);
                return path;
            }
            var steam = Fixture("steam.exe");
            var launcher = Fixture("Kinseat.SteamLauncher.exe");
            Fixture("Kinseat.SteamHook.dll");
            var options = new MultiSeatOptions { SteamIsolationEnabled = true, SteamIsolationLauncherPath = launcher };
            var shortcuts = new GameShortcuts(new FakeStore(), _ => null, () => steam, root,
                options, NullLogger<GameShortcuts>.Instance);
            var game = Game("steam:42", "Steam game", "seat");
            game.Source = "steam";
            game.LaunchUri = "steam://rungameid/42";
            var info = shortcuts.Describe(game)!;
            Assert.Equal(launcher, info.Target);
            Assert.Equal($"--steam \"{steam}\" -- steam://rungameid/42", info.Arguments);
        }
        finally { Directory.Delete(root, true); }
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public void ClearRemovesInventoryWithoutCreatingOne(bool inventoryExists)
    {
        var root = Path.Combine(Path.GetTempPath(), "shortcuts-clear-" + Guid.NewGuid().ToString("N"));
        var inventory = Path.Combine(root, "inventory", "seat.json");
        Directory.CreateDirectory(root);
        try
        {
            if (inventoryExists)
            {
                Directory.CreateDirectory(Path.GetDirectoryName(inventory)!);
                File.WriteAllText(inventory, "[]");
            }
            var shortcuts = new GameShortcuts(new FakeStore(), _ => root, () => "steam.exe",
                Path.GetDirectoryName(inventory)!, new MultiSeatOptions(), NullLogger<GameShortcuts>.Instance);
            shortcuts.Clear("seat");
            Assert.False(File.Exists(inventory));
            shortcuts.Sync("seat", []); // A late empty sync must not recreate the file.
            Assert.False(File.Exists(inventory));
            if (!inventoryExists) Assert.False(Directory.Exists(Path.GetDirectoryName(inventory)));
        }
        finally { Directory.Delete(root, true); }
    }

    [Fact]
    public void ClearRemovesInventoryWhenProfileIsAlreadyGone()
    {
        var root = Path.Combine(Path.GetTempPath(), "shortcuts-gone-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(root);
        try
        {
            var inventory = Path.Combine(root, "seat.json");
            File.WriteAllText(inventory, "[]");
            var shortcuts = new GameShortcuts(new FakeStore(), _ => null, () => "steam.exe", root,
                new MultiSeatOptions(), NullLogger<GameShortcuts>.Instance);
            shortcuts.Clear("seat");
            Assert.False(File.Exists(inventory));
        }
        finally { Directory.Delete(root, true); }
    }

    [Fact]
    public void ClearDeletesOnlyAnOwnedLinkAndItsInventory()
    {
        var root = Path.Combine(Path.GetTempPath(), "shortcuts-owned-" + Guid.NewGuid().ToString("N"));
        var desktop = Path.Combine(root, "Desktop");
        var inventoryDir = Path.Combine(root, "inventory");
        Directory.CreateDirectory(desktop);
        Directory.CreateDirectory(inventoryDir);
        try
        {
            var owned = Path.Combine(desktop, "Owned.lnk");
            var foreign = Path.Combine(desktop, "Player.lnk");
            File.WriteAllText(owned, "managed");
            File.WriteAllText(foreign, "player");
            var inventory = Path.Combine(inventoryDir, "seat.json");
            File.WriteAllText(inventory, System.Text.Json.JsonSerializer.Serialize(new[]
            {
                new GameShortcuts.OwnedLink(owned, "game.exe", "game")
            }));
            var shortcuts = new GameShortcuts(new OwnedStore(owned), _ => root, () => "steam.exe", inventoryDir,
                new MultiSeatOptions(), NullLogger<GameShortcuts>.Instance);
            shortcuts.Clear("seat");
            Assert.False(File.Exists(owned));
            Assert.Equal("player", File.ReadAllText(foreign));
            Assert.False(File.Exists(inventory));
        }
        finally { Directory.Delete(root, true); }
    }

    private sealed class OwnedStore(string owned) : IShortcutStore
    {
        public ShortcutInfo? Read(string path) => path == owned ? new ShortcutInfo("game.exe", "", "", "", "") : null;
        public void Write(string path, ShortcutInfo info) => throw new NotSupportedException();
    }

    private sealed class FakeStore : IShortcutStore
    {
        public ShortcutInfo? Read(string path) => null;
        public void Write(string path, ShortcutInfo info) => File.WriteAllText(path, "managed");
    }
}
