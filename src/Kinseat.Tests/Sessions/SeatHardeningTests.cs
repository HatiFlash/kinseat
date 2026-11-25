using System.Text.Json;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Logging.Abstractions;
using MultiSeat.Service.Sessions;
using Xunit;

namespace MultiSeat.Tests.Sessions;

public sealed class SeatHardeningTests
{
    private const string Seat = "S-1-5-21-1-2-3-1001";
    private const string Seat2 = "S-1-5-21-1-2-3-1005";
    private const string ConsoleUser = "S-1-5-21-1-2-3-1002";
    private const string FamilyLocal = "S-1-5-21-1-2-3-1003";
    private const string MicrosoftEntra = "S-1-12-1-900-1000-2000-3000";
    private const string DomainUser = "S-1-5-21-9-8-7-1104";
    private const string DedicatedGroupSid = "S-1-5-21-1-2-3-2000";
    private static readonly HashSet<string> Seats = new(StringComparer.OrdinalIgnoreCase) { Seat };

    private sealed class FakeLsa : ShutdownRightPolicy.ILsaPolicy
    {
        internal readonly Dictionary<string, HashSet<string>> Rights = new(StringComparer.Ordinal);
        internal readonly List<string> Calls = [];
        internal int FailOnCall;
        public string[] Enumerate(string right) => Rights.TryGetValue(right, out var s) ? s.ToArray() : [];
        public void Add(string right, string sid) => Update(right, sid, true);
        public void Remove(string right, string sid) => Update(right, sid, false);
        private void Update(string right, string sid, bool add)
        {
            Calls.Add($"{(add ? "add" : "remove")}:{right}:{sid}");
            if (FailOnCall == Calls.Count) throw new IOException("LSA interrupted");
            if (!Rights.TryGetValue(right, out var values)) Rights[right] = values = new(StringComparer.OrdinalIgnoreCase);
            if (add) values.Add(sid); else values.Remove(sid);
        }
        public void Dispose() { }
    }

    private sealed class FakeLocalGroups : ShutdownRightPolicy.ILocalGroupPolicy
    {
        internal readonly Dictionary<string, HashSet<string>> Groups = new(StringComparer.OrdinalIgnoreCase);
        internal string GroupSid { get; set; } = DedicatedGroupSid;
        internal string? FailOnAddSid { get; set; }
        internal bool Deleted { get; private set; }

        public string EnsureGroup(string name, string comment)
        {
            if (!Groups.ContainsKey(name)) Groups[name] = new(StringComparer.OrdinalIgnoreCase);
            return GroupSid;
        }

        public string[] GetMemberSids(string groupName) =>
            Groups.TryGetValue(groupName, out var members) ? members.ToArray() : [];

        public void AddMemberSid(string groupName, string sid)
        {
            if (string.Equals(FailOnAddSid, sid, StringComparison.OrdinalIgnoreCase))
                throw new InvalidOperationException($"Add failed for {sid}");
            if (!Groups.TryGetValue(groupName, out var members)) Groups[groupName] = members = new(StringComparer.OrdinalIgnoreCase);
            members.Add(sid);
        }

        public void RemoveMemberSid(string groupName, string sid)
        {
            if (Groups.TryGetValue(groupName, out var members)) members.Remove(sid);
        }

        public void DeleteGroup(string name)
        {
            Groups.Remove(name);
            Deleted = true;
        }
    }

    private sealed class CapturingLogger : ILogger
    {
        internal readonly List<(LogLevel Level, string Message)> Entries = [];
        public IDisposable? BeginScope<TState>(TState state) where TState : notnull => null;
        public bool IsEnabled(LogLevel logLevel) => true;
        public void Log<TState>(LogLevel logLevel, EventId eventId, TState state, Exception? exception, Func<TState, Exception?, string> formatter) =>
            Entries.Add((logLevel, formatter(state, exception)));
    }

    private static string TempStatePath() =>
        Path.Combine(Path.GetTempPath(), $"kinseat-rights-{Guid.NewGuid():N}.json");

    [Fact]
    public void ShutdownPlanGrantsDedicatedGroupBeforeRemovingOnlyBroadGroups()
    {
        var original = new Dictionary<string, string[]>
        {
            ["SeShutdownPrivilege"] = ["S-1-5-32-544", "S-1-5-32-551", "S-1-5-32-545", "S-1-5-4", "S-1-5-18"],
            ["SeRemoteShutdownPrivilege"] = ["S-1-5-32-544", "S-1-5-11"]
        };
        var changes = ShutdownRightPolicy.Plan(original, DedicatedGroupSid);
        Assert.Equal(["S-1-5-32-545", "S-1-5-4"], changes[0].Remove);
        Assert.Equal([DedicatedGroupSid], changes[0].Add);
        Assert.Equal(["S-1-5-11"], changes[1].Remove);
        Assert.Equal([DedicatedGroupSid], changes[1].Add);
        Assert.DoesNotContain(changes.SelectMany(c => c.Remove), sid => sid is "S-1-5-32-544" or "S-1-5-32-551" or "S-1-5-18");
    }

    [Fact]
    public void ShutdownPlanAddsAdministratorsWhenMissingAndSkipsRightWithoutBroadGroups()
    {
        var original = new Dictionary<string, string[]>
        {
            ["SeShutdownPrivilege"] = ["S-1-5-32-545"],
            ["SeRemoteShutdownPrivilege"] = ["S-1-5-32-544", "S-1-5-32-551"]
        };
        var changes = ShutdownRightPolicy.Plan(original, DedicatedGroupSid);
        Assert.Equal([DedicatedGroupSid, "S-1-5-32-544"], changes[0].Add);
        Assert.Equal(["S-1-5-32-545"], changes[0].Remove);
        Assert.Empty(changes[1].Add);
        Assert.Empty(changes[1].Remove);
    }

    [Fact]
    public void ApplyProtectsLocalFamilyMicrosoftAndDomainAccountsWhileExcludingSeat()
    {
        var path = TempStatePath();
        var lsa = new FakeLsa();
        lsa.Rights["SeShutdownPrivilege"] = new(["S-1-5-32-544", "S-1-5-32-551", "S-1-5-32-545", "S-1-1-0"], StringComparer.OrdinalIgnoreCase);
        lsa.Rights["SeRemoteShutdownPrivilege"] = new(["S-1-5-32-544"], StringComparer.OrdinalIgnoreCase);
        var groups = new FakeLocalGroups();
        var logger = new CapturingLogger();
        var host = new ShutdownRightPolicy.HostAccounts(
            ActiveLocalUsers: new HashSet<string>(StringComparer.OrdinalIgnoreCase) { Seat, ConsoleUser, FamilyLocal },
            ConsoleUserSid: ConsoleUser,
            NonLocalProfileSids: new HashSet<string>(StringComparer.OrdinalIgnoreCase) { MicrosoftEntra, DomainUser },
            PrivilegedMemberSids: new HashSet<string>(StringComparer.OrdinalIgnoreCase));
        try
        {
            Assert.True(ShutdownRightPolicy.ApplyCore(lsa, groups, path, Seats, host, logger));
            var members = groups.GetMemberSids(ShutdownRightPolicy.AllowedGroupName);
            Assert.Contains(ConsoleUser, members);
            Assert.Contains(FamilyLocal, members);
            Assert.Contains(MicrosoftEntra, members);
            Assert.Contains(DomainUser, members);
            Assert.DoesNotContain(Seat, members);

            var shutdownGrants = lsa.Enumerate("SeShutdownPrivilege");
            Assert.Contains(DedicatedGroupSid, shutdownGrants);
            Assert.Contains("S-1-5-32-544", shutdownGrants);
            Assert.Contains("S-1-5-32-551", shutdownGrants);
            Assert.DoesNotContain("S-1-5-32-545", shutdownGrants);
            Assert.DoesNotContain("S-1-1-0", shutdownGrants);
            Assert.True(lsa.Calls.IndexOf($"add:SeShutdownPrivilege:{DedicatedGroupSid}") <
                lsa.Calls.IndexOf("remove:SeShutdownPrivilege:S-1-5-32-545"));
        }
        finally { File.Delete(path); File.Delete(path + ".tmp"); }
    }

    [Fact]
    public void FailedGroupMemberAddAbortsBeforeRemovingAnyLsaRight()
    {
        var path = TempStatePath();
        var lsa = new FakeLsa();
        lsa.Rights["SeShutdownPrivilege"] = new(["S-1-5-32-544", "S-1-5-32-545"], StringComparer.OrdinalIgnoreCase);
        lsa.Rights["SeRemoteShutdownPrivilege"] = new(["S-1-5-32-544"], StringComparer.OrdinalIgnoreCase);
        var groups = new FakeLocalGroups { FailOnAddSid = MicrosoftEntra };
        var logger = new CapturingLogger();
        var host = new ShutdownRightPolicy.HostAccounts(
            ActiveLocalUsers: new HashSet<string>(StringComparer.OrdinalIgnoreCase) { Seat, ConsoleUser },
            ConsoleUserSid: ConsoleUser,
            NonLocalProfileSids: new HashSet<string>(StringComparer.OrdinalIgnoreCase) { MicrosoftEntra },
            PrivilegedMemberSids: new HashSet<string>(StringComparer.OrdinalIgnoreCase));
        try
        {
            Assert.False(ShutdownRightPolicy.ApplyCore(lsa, groups, path, Seats, host, logger));
            Assert.Empty(lsa.Calls);
            Assert.Contains("S-1-5-32-545", lsa.Enumerate("SeShutdownPrivilege"));
            Assert.False(File.Exists(path));
            Assert.Contains(logger.Entries, e => e.Level == LogLevel.Warning && e.Message.Contains(MicrosoftEntra));
        }
        finally { File.Delete(path); File.Delete(path + ".tmp"); }
    }

    [Fact]
    public void SeatInBackupOperatorsLogsWarningAndIsNotTreatedAsProtected()
    {
        var path = TempStatePath();
        var lsa = new FakeLsa();
        lsa.Rights["SeShutdownPrivilege"] = new(["S-1-5-32-544", "S-1-5-32-551", "S-1-5-32-545"], StringComparer.OrdinalIgnoreCase);
        lsa.Rights["SeRemoteShutdownPrivilege"] = new(["S-1-5-32-544"], StringComparer.OrdinalIgnoreCase);
        var groups = new FakeLocalGroups();
        var logger = new CapturingLogger();
        var host = new ShutdownRightPolicy.HostAccounts(
            ActiveLocalUsers: new HashSet<string>(StringComparer.OrdinalIgnoreCase) { Seat, ConsoleUser },
            ConsoleUserSid: ConsoleUser,
            NonLocalProfileSids: new HashSet<string>(StringComparer.OrdinalIgnoreCase),
            PrivilegedMemberSids: new HashSet<string>(StringComparer.OrdinalIgnoreCase) { Seat });
        try
        {
            // Only seat is in Backup Operators: warn and do not treat as protected.
            Assert.False(ShutdownRightPolicy.ApplyCore(lsa, groups, path, Seats, host, logger));
            Assert.Empty(lsa.Calls);
            Assert.Contains(logger.Entries, e => e.Level == LogLevel.Warning && e.Message.Contains(Seat));

            // With a second normal seat: privileged seat is still excluded from dedicated group, normal seat is protected.
            var bothSeats = new HashSet<string>(StringComparer.OrdinalIgnoreCase) { Seat, Seat2 };
            var host2 = host with { ActiveLocalUsers = new HashSet<string>(StringComparer.OrdinalIgnoreCase) { Seat, Seat2, ConsoleUser } };
            Assert.True(ShutdownRightPolicy.ApplyCore(lsa, groups, path, bothSeats, host2, logger));
            var members = groups.GetMemberSids(ShutdownRightPolicy.AllowedGroupName);
            Assert.DoesNotContain(Seat, members);
            Assert.DoesNotContain(Seat2, members);
            Assert.Contains(ConsoleUser, members);
            Assert.Contains("S-1-5-32-551", lsa.Enumerate("SeShutdownPrivilege"));
        }
        finally { File.Delete(path); File.Delete(path + ".tmp"); }
    }

    [Fact]
    public void ApplyResumeIdempotenceAndRestorePreserveOriginalJournal()
    {
        var path = TempStatePath();
        var original = new Dictionary<string, string[]>
        {
            ["SeShutdownPrivilege"] = ["S-1-5-32-545"],
            ["SeRemoteShutdownPrivilege"] = ["S-1-5-11"]
        };
        var lsa = new FakeLsa { FailOnCall = 2 };
        foreach (var right in original) lsa.Rights[right.Key] = right.Value.ToHashSet(StringComparer.OrdinalIgnoreCase);
        var groups = new FakeLocalGroups();
        var host = new ShutdownRightPolicy.HostAccounts(
            ActiveLocalUsers: new HashSet<string>(StringComparer.OrdinalIgnoreCase) { Seat, ConsoleUser },
            ConsoleUserSid: ConsoleUser,
            NonLocalProfileSids: new HashSet<string>(StringComparer.OrdinalIgnoreCase),
            PrivilegedMemberSids: new HashSet<string>(StringComparer.OrdinalIgnoreCase));
        try
        {
            Assert.Throws<IOException>(() => ShutdownRightPolicy.ApplyCore(lsa, groups, path, Seats, host, NullLogger.Instance));
            var pending = JsonSerializer.Deserialize<ShutdownRightPolicy.Snapshot>(File.ReadAllText(path))!;
            Assert.False(pending.Applied);

            // Resume on restart: completes remaining LSA mutations and marks Applied = true.
            lsa.FailOnCall = 0;
            Assert.True(ShutdownRightPolicy.ApplyCore(lsa, groups, path, Seats, host, NullLogger.Instance));
            var completedJson = File.ReadAllText(path);
            var completed = JsonSerializer.Deserialize<ShutdownRightPolicy.Snapshot>(completedJson)!;
            Assert.True(completed.Applied);
            Assert.Equal(ShutdownRightPolicy.AllowedGroupName, completed.GroupName);
            Assert.Equal(DedicatedGroupSid, completed.GroupSid);
            Assert.True(lsa.Calls.IndexOf($"add:SeShutdownPrivilege:{DedicatedGroupSid}") <
                lsa.Calls.IndexOf("remove:SeShutdownPrivilege:S-1-5-32-545"));

            // Idempotent pass with a newly added family account: updates group membership without overwriting original journal.
            var hostWithFamily = host with { ActiveLocalUsers = new HashSet<string>(StringComparer.OrdinalIgnoreCase) { Seat, ConsoleUser, FamilyLocal } };
            Assert.True(ShutdownRightPolicy.ApplyCore(lsa, groups, path, Seats, hostWithFamily, NullLogger.Instance));
            Assert.Equal(completedJson, File.ReadAllText(path));
            Assert.Contains(FamilyLocal, groups.GetMemberSids(ShutdownRightPolicy.AllowedGroupName));

            // Restore when no seats remain: restores broad groups, removes dedicated group & state.json.
            Assert.True(ShutdownRightPolicy.ApplyCore(lsa, groups, path, new HashSet<string>(), hostWithFamily, NullLogger.Instance));
            Assert.False(File.Exists(path));
            Assert.True(groups.Deleted);
            Assert.Equal(original["SeShutdownPrivilege"], lsa.Enumerate("SeShutdownPrivilege"));
            Assert.Equal(original["SeRemoteShutdownPrivilege"], lsa.Enumerate("SeRemoteShutdownPrivilege"));
        }
        finally { File.Delete(path); File.Delete(path + ".tmp"); }
    }

    [Fact]
    public void InvalidSnapshotNeverMutatesLsa()
    {
        var lsa = new FakeLsa();
        var path = TempStatePath();
        var invalid = new ShutdownRightPolicy.Snapshot([new("SeShutdownPrivilege", [Seat], [])]);
        Assert.False(ShutdownRightPolicy.ValidateSnapshot(invalid));
        Assert.Throws<InvalidDataException>(() => ShutdownRightPolicy.ApplySnapshot(lsa, path, invalid));
        Assert.Empty(lsa.Calls);
    }

    [Fact]
    public void NonLocalProfileSidFilterMatchesEntraAndDomainOnly()
    {
        var prefix = ShutdownRightPolicy.ExtractMachineSidPrefix(DedicatedGroupSid);
        Assert.Equal("S-1-5-21-1-2-3-", prefix);
        Assert.True(ShutdownRightPolicy.IsNonLocalProfileSid(MicrosoftEntra, prefix));
        Assert.True(ShutdownRightPolicy.IsNonLocalProfileSid(DomainUser, prefix));
        Assert.False(ShutdownRightPolicy.IsNonLocalProfileSid(ConsoleUser, prefix));
        Assert.False(ShutdownRightPolicy.IsNonLocalProfileSid("S-1-5-18", prefix));
        Assert.False(ShutdownRightPolicy.IsNonLocalProfileSid("S-1-5-21-9-8-7-501", prefix));
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
        var native = SeatShortcutPolicy.SelectShortcuts([
            Path.Combine("Desktop", "Kinseat Hôte.lnk"), Path.Combine("Desktop", "HidHide Configurator.lnk"),
            Path.Combine("Desktop", "Jeu.lnk"), Path.Combine("Desktop", "KINSEAT HÔTE.LNK")
        ]);
        Assert.Equal(2, native.Length);
        Assert.All(native, name => Assert.DoesNotContain("Jeu", name));
        if (OperatingSystem.IsWindows()) Assert.Equal(2, selected.Length);
    }
}
