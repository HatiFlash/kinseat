using System.Text.Json;
using MultiSeat.Service.Sessions;
using Xunit;

namespace MultiSeat.Tests.Sessions;

public sealed class SeatHardeningTests
{
    private const string Seat = "S-1-5-21-1-2-3-1001";
    private const string Console = "S-1-5-21-1-2-3-1002";
    private static readonly HashSet<string> LocalUsers = new(StringComparer.OrdinalIgnoreCase) { Seat, Console };
    private static readonly HashSet<string> Seats = new(StringComparer.OrdinalIgnoreCase) { Seat };

    [Fact]
    public void ShutdownPlanPreservesAdministratorsAndConsoleAndRemovesSharedGrants()
    {
        var original = new Dictionary<string, string[]>
        {
            ["SeShutdownPrivilege"] = ["S-1-5-32-544", "S-1-5-32-545", "S-1-5-4"],
            ["SeRemoteShutdownPrivilege"] = ["S-1-5-32-544", "S-1-5-11"]
        };
        var changes = ShutdownRightPolicy.Plan(original, Seats, LocalUsers);
        Assert.Equal(["S-1-5-32-545", "S-1-5-4"], changes[0].Remove);
        Assert.Equal([Console], changes[0].Add);
        Assert.Equal(["S-1-5-11"], changes[1].Remove);
        Assert.Empty(changes[1].Add);
        Assert.DoesNotContain(changes.SelectMany(c => c.Remove), sid => sid == "S-1-5-32-544");
        Assert.DoesNotContain(changes.SelectMany(c => c.Add), sid => sid == Seat);
        Assert.Equal(["S-1-5-32-545"], ShutdownRightPolicy.RestoreTargets(
            ["S-1-5-32-544"], original["SeShutdownPrivilege"][..2]));
    }

    [Fact]
    public void ShutdownPlanAddsAdministratorsBeforeRemovingSharedGrant()
    {
        var original = new Dictionary<string, string[]>
        {
            ["SeShutdownPrivilege"] = ["S-1-5-32-545"],
            ["SeRemoteShutdownPrivilege"] = ["S-1-5-11"]
        };
        var plan = ShutdownRightPolicy.Plan(original, Seats, LocalUsers);
        Assert.Contains("S-1-5-32-544", plan[0].Add);
        Assert.Contains("S-1-5-32-544", plan[1].Add);
        Assert.DoesNotContain(Seat, plan[0].Add);
    }

    [Fact]
    public void ShutdownPlanRefusesUnknownGroupOrDirectSeatGrant()
    {
        var original = new Dictionary<string, string[]>
        {
            ["SeShutdownPrivilege"] = ["S-1-5-32-544", "S-1-5-32-551"],
            ["SeRemoteShutdownPrivilege"] = ["S-1-5-32-544"]
        };
        Assert.Throws<InvalidOperationException>(() => ShutdownRightPolicy.Plan(original, Seats, LocalUsers));
        original["SeShutdownPrivilege"] = ["S-1-5-32-544", Seat];
        Assert.Throws<InvalidOperationException>(() => ShutdownRightPolicy.Plan(original, Seats, LocalUsers));
    }

    [Fact]
    public void SnapshotRoundTripRetainsRestorePlanWithoutOverwritingUnrelatedRights()
    {
        var saved = new ShutdownRightPolicy.Snapshot([
            new("SeShutdownPrivilege", ["S-1-5-32-545"], [Console]),
            new("SeRemoteShutdownPrivilege", ["S-1-5-11"], [])
        ]);
        var restored = JsonSerializer.Deserialize<ShutdownRightPolicy.Snapshot>(JsonSerializer.Serialize(saved))!;
        Assert.Equal(saved.Changes[0].Remove, restored.Changes[0].Remove);
        Assert.Equal(saved.Changes[0].Add, restored.Changes[0].Add);
        Assert.Empty(ShutdownRightPolicy.RestoreTargets(["S-1-5-32-545", "S-1-5-32-544"],
            restored.Changes[0].Remove));
        Assert.Equal(["S-1-5-11"], ShutdownRightPolicy.RestoreTargets(["S-1-5-32-544"],
            restored.Changes[1].Remove));
    }

    [Fact]
    public void ShortcutSelectionLeavesGamesAloneAndDeduplicates()
    {
        var selected = SeatShortcutPolicy.SelectShortcuts([
            @"C:\Users\Public\Desktop\Kinseat Hôte.lnk",
            @"C:\Users\Public\Desktop\HidHide Configurator.lnk",
            @"C:\Users\Public\Desktop\Jeu.lnk",
            @"C:\Users\Public\Desktop\KINSEAT HÔTE.LNK"
        ]);
        // Path.GetFileName uses the host platform's separators; test with platform-native paths.
        var native = SeatShortcutPolicy.SelectShortcuts([
            Path.Combine("Desktop", "Kinseat Hôte.lnk"), Path.Combine("Desktop", "HidHide Configurator.lnk"),
            Path.Combine("Desktop", "Jeu.lnk"), Path.Combine("Desktop", "KINSEAT HÔTE.LNK")
        ]);
        Assert.Equal(2, native.Length);
        Assert.All(native, name => Assert.DoesNotContain("Jeu", name));
        if (OperatingSystem.IsWindows()) Assert.Equal(2, selected.Length);
    }
}
