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
        internal string? FailOnRemoveSid { get; set; }
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
            if (string.Equals(FailOnRemoveSid, sid, StringComparison.OrdinalIgnoreCase))
                throw new InvalidOperationException($"Remove failed for {sid}");
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

            // With a second normal seat: privileged seat stays unprotected (reconciliation fails, no false success),
            // while both seats are excluded from dedicated group and Backup Operators is untouched.
            var bothSeats = new HashSet<string>(StringComparer.OrdinalIgnoreCase) { Seat, Seat2 };
            var host2 = host with { ActiveLocalUsers = new HashSet<string>(StringComparer.OrdinalIgnoreCase) { Seat, Seat2, ConsoleUser } };
            var result = ShutdownRightPolicy.ReconcileCore(lsa, groups, path, bothSeats, host2, logger);
            Assert.False(result.Succeeded);
            Assert.Contains(Seat, result.UnprotectedSeats);
            Assert.DoesNotContain(Seat2, result.UnprotectedSeats);
            Assert.Contains(logger.Entries, e => e.Level == LogLevel.Warning && e.Message.Contains(Seat) && e.Message.Contains("non protégée"));
            var members = groups.GetMemberSids(ShutdownRightPolicy.AllowedGroupName);
            Assert.DoesNotContain(Seat, members);
            Assert.DoesNotContain(Seat2, members);
            Assert.Contains(ConsoleUser, members);
            Assert.Contains("S-1-5-32-551", lsa.Enumerate("SeShutdownPrivilege"));
        }
        finally { File.Delete(path); File.Delete(path + ".tmp"); }
    }

    [Fact]
    public void ReconciliationNeverRemovesNonSeatMembersAndAddsNewLocalAndProfileListAccounts()
    {
        var path = TempStatePath();
        var lsa = new FakeLsa();
        lsa.Rights["SeShutdownPrivilege"] = new(["S-1-5-32-544", "S-1-5-32-545"], StringComparer.OrdinalIgnoreCase);
        lsa.Rights["SeRemoteShutdownPrivilege"] = new(["S-1-5-32-544"], StringComparer.OrdinalIgnoreCase);
        var groups = new FakeLocalGroups();
        var manualNonSeat = "S-1-5-21-1-2-3-1999";
        var previousDomainMember = "S-1-5-21-9-8-7-1200";
        groups.EnsureGroup(ShutdownRightPolicy.AllowedGroupName, "test");
        groups.AddMemberSid(ShutdownRightPolicy.AllowedGroupName, manualNonSeat);
        groups.AddMemberSid(ShutdownRightPolicy.AllowedGroupName, previousDomainMember);
        groups.AddMemberSid(ShutdownRightPolicy.AllowedGroupName, Seat);
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
            Assert.Contains(manualNonSeat, members);
            Assert.Contains(previousDomainMember, members);
            Assert.Contains(ConsoleUser, members);
            Assert.Contains(FamilyLocal, members);
            Assert.Contains(MicrosoftEntra, members);
            Assert.Contains(DomainUser, members);
            Assert.DoesNotContain(Seat, members);
            Assert.True(MultiSeat.Service.MultiSeatWorker.ShutdownPolicyReconcileInterval > TimeSpan.Zero);
            Assert.True(MultiSeat.Service.MultiSeatWorker.ShutdownPolicyReconcileInterval <= TimeSpan.FromSeconds(60));
        }
        finally { File.Delete(path); File.Delete(path + ".tmp"); }
    }

    [Fact]
    public void DirectSeatLsaGrantMarksSeatUnprotectedAndFailsReconciliation()
    {
        var path = TempStatePath();
        var lsa = new FakeLsa();
        lsa.Rights["SeShutdownPrivilege"] = new(["S-1-5-32-544", "S-1-5-32-545", Seat], StringComparer.OrdinalIgnoreCase);
        lsa.Rights["SeRemoteShutdownPrivilege"] = new(["S-1-5-32-544"], StringComparer.OrdinalIgnoreCase);
        var groups = new FakeLocalGroups();
        var logger = new CapturingLogger();
        var host = new ShutdownRightPolicy.HostAccounts(
            ActiveLocalUsers: new HashSet<string>(StringComparer.OrdinalIgnoreCase) { Seat, ConsoleUser },
            ConsoleUserSid: ConsoleUser,
            NonLocalProfileSids: new HashSet<string>(StringComparer.OrdinalIgnoreCase),
            PrivilegedMemberSids: new HashSet<string>(StringComparer.OrdinalIgnoreCase));
        try
        {
            var first = ShutdownRightPolicy.ReconcileCore(lsa, groups, path, Seats, host, logger);
            Assert.False(first.Succeeded);
            Assert.Contains(Seat, first.UnprotectedSeats);
            Assert.Contains(Seat, lsa.Enumerate("SeShutdownPrivilege"));
            Assert.Contains(logger.Entries, e => e.Level == LogLevel.Warning && e.Message.Contains(Seat) && e.Message.Contains("non protégée"));

            // Subsequent reconciliation with existing state.json still reports failure while direct grant remains.
            logger.Entries.Clear();
            var second = ShutdownRightPolicy.ReconcileCore(lsa, groups, path, Seats, host, logger);
            Assert.False(second.Succeeded);
            Assert.Contains(Seat, second.UnprotectedSeats);
            Assert.Contains(logger.Entries, e => e.Level == LogLevel.Warning && e.Message.Contains(Seat) && e.Message.Contains("non protégée"));
        }
        finally { File.Delete(path); File.Delete(path + ".tmp"); }
    }

    [Fact]
    public void FailedSeatRemovalFromDedicatedGroupMarksSeatUnprotectedAndFailsReconciliation()
    {
        var path = TempStatePath();
        var lsa = new FakeLsa();
        lsa.Rights["SeShutdownPrivilege"] = new(["S-1-5-32-544", "S-1-5-32-545"], StringComparer.OrdinalIgnoreCase);
        lsa.Rights["SeRemoteShutdownPrivilege"] = new(["S-1-5-32-544"], StringComparer.OrdinalIgnoreCase);
        var groups = new FakeLocalGroups { FailOnRemoveSid = Seat };
        groups.EnsureGroup(ShutdownRightPolicy.AllowedGroupName, "test");
        groups.AddMemberSid(ShutdownRightPolicy.AllowedGroupName, Seat);
        var logger = new CapturingLogger();
        var host = new ShutdownRightPolicy.HostAccounts(
            ActiveLocalUsers: new HashSet<string>(StringComparer.OrdinalIgnoreCase) { Seat, ConsoleUser },
            ConsoleUserSid: ConsoleUser,
            NonLocalProfileSids: new HashSet<string>(StringComparer.OrdinalIgnoreCase),
            PrivilegedMemberSids: new HashSet<string>(StringComparer.OrdinalIgnoreCase));
        try
        {
            var result = ShutdownRightPolicy.ReconcileCore(lsa, groups, path, Seats, host, logger);
            Assert.False(result.Succeeded);
            Assert.Contains(Seat, result.UnprotectedSeats);
            Assert.Contains(Seat, groups.GetMemberSids(ShutdownRightPolicy.AllowedGroupName));
            Assert.Contains(logger.Entries, e => e.Level == LogLevel.Warning && e.Message.Contains(Seat) && e.Message.Contains("non protégée"));
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
    public void UpgradeKeepsInitialBaselineAndRestoresGroupEvenWhenExportedOnOtherRight()
    {
        var path = TempStatePath();
        var lsa = new FakeLsa();
        lsa.Rights["SeShutdownPrivilege"] = new([ShutdownRightPolicy.AdminsSid, ShutdownRightPolicy.BackupOperatorsSid, "S-1-5-32-545"], StringComparer.OrdinalIgnoreCase);
        lsa.Rights["SeRemoteShutdownPrivilege"] = new([ShutdownRightPolicy.AdminsSid], StringComparer.OrdinalIgnoreCase);
        var groups = new FakeLocalGroups();
        var host = new ShutdownRightPolicy.HostAccounts(new HashSet<string> { Seat, ConsoleUser }, ConsoleUser,
            new HashSet<string>(), new HashSet<string>());
        try
        {
            Assert.True(ShutdownRightPolicy.ApplyCore(lsa, groups, path, Seats, host, NullLogger.Instance));
            var baseline = File.ReadAllText(path);
            // Upgrade/reconciliation with a group grant that was not in the original plan.
            lsa.Rights["SeRemoteShutdownPrivilege"].Add(DedicatedGroupSid);
            Assert.True(ShutdownRightPolicy.ApplyCore(lsa, groups, path, Seats, host, NullLogger.Instance));
            Assert.Equal(baseline, File.ReadAllText(path));
            ShutdownRightPolicy.Restore(lsa, groups, path);
            Assert.Equal([ShutdownRightPolicy.AdminsSid, ShutdownRightPolicy.BackupOperatorsSid, "S-1-5-32-545"],
                lsa.Enumerate("SeShutdownPrivilege"));
            Assert.Equal([ShutdownRightPolicy.AdminsSid], lsa.Enumerate("SeRemoteShutdownPrivilege"));
            Assert.True(groups.Deleted);
            Assert.True(lsa.Calls.IndexOf($"remove:SeRemoteShutdownPrivilege:{DedicatedGroupSid}") >= 0);
            var calls = lsa.Calls.Count;
            ShutdownRightPolicy.Restore(lsa, groups, path);
            Assert.Equal(calls, lsa.Calls.Count);
        }
        finally { File.Delete(path); File.Delete(path + ".tmp"); }
    }

    [Fact]
    public void RestoreFailureLeavesGroupAndJournalForRetry()
    {
        var path = TempStatePath();
        var lsa = new FakeLsa();
        lsa.Rights["SeShutdownPrivilege"] = new([ShutdownRightPolicy.AdminsSid, "S-1-5-32-545"], StringComparer.OrdinalIgnoreCase);
        lsa.Rights["SeRemoteShutdownPrivilege"] = new([ShutdownRightPolicy.AdminsSid], StringComparer.OrdinalIgnoreCase);
        var groups = new FakeLocalGroups();
        var host = new ShutdownRightPolicy.HostAccounts(new HashSet<string> { Seat, ConsoleUser }, ConsoleUser,
            new HashSet<string>(), new HashSet<string>());
        try
        {
            Assert.True(ShutdownRightPolicy.ApplyCore(lsa, groups, path, Seats, host, NullLogger.Instance));
            lsa.FailOnCall = lsa.Calls.Count + 1;
            Assert.Throws<IOException>(() => ShutdownRightPolicy.Restore(lsa, groups, path));
            Assert.True(File.Exists(path));
            Assert.False(groups.Deleted);
            lsa.FailOnCall = 0;
            ShutdownRightPolicy.Restore(lsa, groups, path);
            Assert.False(File.Exists(path));
            Assert.True(groups.Deleted);
        }
        finally { File.Delete(path); File.Delete(path + ".tmp"); }
    }

    [Fact]
    public void MissingJournalNeverRecapturesAlreadyModifiedLsaBaseline()
    {
        var path = TempStatePath();
        var lsa = new FakeLsa();
        lsa.Rights["SeShutdownPrivilege"] = new([ShutdownRightPolicy.AdminsSid, DedicatedGroupSid], StringComparer.OrdinalIgnoreCase);
        lsa.Rights["SeRemoteShutdownPrivilege"] = new([ShutdownRightPolicy.AdminsSid], StringComparer.OrdinalIgnoreCase);
        var groups = new FakeLocalGroups();
        var host = new ShutdownRightPolicy.HostAccounts(new HashSet<string> { Seat, ConsoleUser }, ConsoleUser,
            new HashSet<string>(), new HashSet<string>());
        Assert.Throws<InvalidOperationException>(() =>
            ShutdownRightPolicy.ApplyCore(lsa, groups, path, Seats, host, NullLogger.Instance));
        Assert.False(File.Exists(path));
        Assert.Empty(lsa.Calls);
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

    private sealed class FakeShortcutAclHandler : SeatShortcutPolicy.IShortcutAclHandler
    {
        internal readonly List<string> RestrictedDirectories = [];
        internal readonly List<string> RestrictedFiles = [];
        internal readonly List<(string Path, string Sid)> RemovedLegacyDenyAces = [];
        internal readonly List<(string Path, string[] Sids)> StrippedBeforeMove = [];
        internal readonly List<(string Path, string? Sddl)> RestoredAcls = [];
        internal string CapturedSddl { get; set; } = "D:AI(A;;FA;;;SY)(A;;FA;;;BA)(A;;0x1200a9;;;BU)";
        internal Func<string, bool>? FailRestrictPredicate { get; set; }

        public void EnsureRestrictedDirectory(string directoryPath)
        {
            Directory.CreateDirectory(directoryPath);
            RestrictedDirectories.Add(directoryPath);
        }

        public string? CaptureAclAndStripLegacyDeny(string filePath, IReadOnlyCollection<string> legacyDenySids, ILogger logger)
        {
            StrippedBeforeMove.Add((filePath, legacyDenySids.ToArray()));
            return CapturedSddl;
        }

        public void RestrictBackupFile(string backupPath)
        {
            RestrictedFiles.Add(backupPath);
            if (FailRestrictPredicate?.Invoke(backupPath) == true)
                throw new UnauthorizedAccessException($"ACL restriction failed for {backupPath}");
        }

        public void RestoreFileAcl(string filePath, string? originalSddl, ILogger logger) =>
            RestoredAcls.Add((filePath, originalSddl));

        public void RemoveLegacyDenyAce(string filePath, string sid, ILogger logger) =>
            RemovedLegacyDenyAces.Add((filePath, sid));
    }

    private sealed class TempShortcutSandbox : IDisposable
    {
        internal string Root { get; } = Path.Combine(Path.GetTempPath(), $"kinseat-shortcuts-{Guid.NewGuid():N}");
        internal string Desktop { get; }
        internal string StartMenu { get; }
        internal string Programs { get; }
        internal string Outside { get; }
        internal string Store { get; }
        internal string ManifestPath { get; }
        internal string BackupDir { get; }
        internal SeatShortcutPolicy.PolicyPaths Paths { get; }

        internal TempShortcutSandbox()
        {
            Desktop = Path.Combine(Root, "PublicDesktop");
            StartMenu = Path.Combine(Root, "CommonStartMenu");
            Programs = Path.Combine(StartMenu, "Programs");
            Outside = Path.Combine(Root, "Outside");
            Store = Path.Combine(Root, "shutdown-rights");
            ManifestPath = Path.Combine(Store, "shortcuts.json");
            BackupDir = Path.Combine(Store, "shortcuts-backup");
            Directory.CreateDirectory(Desktop);
            Directory.CreateDirectory(Programs);
            Directory.CreateDirectory(Outside);
            Paths = new SeatShortcutPolicy.PolicyPaths([Desktop, StartMenu, Programs], ManifestPath, BackupDir);
        }

        public void Dispose()
        {
            try { if (Directory.Exists(Root)) Directory.Delete(Root, recursive: true); }
            catch { }
        }
    }

    [Fact]
    public void ShortcutSelectionMatchesOnlyExactSafeNamesAndDeduplicates()
    {
        var selected = SeatShortcutPolicy.SelectShortcuts([
            @"C:\Users\Public\Desktop\Kinseat Hôte.lnk",
            @"C:\Users\Public\Desktop\Kinseat HÃ´te.lnk",
            @"C:\Users\Public\Desktop\HidHide Configurator.lnk",
            @"C:\Users\Public\Desktop\HidHide Configuration Client.lnk",
            @"C:\Users\Public\Desktop\Jeu.lnk",
            @"C:\Users\Public\Desktop\Kinseat Hôte 2.lnk",
            @"C:\Users\Public\Desktop\HidHide.lnk",
            @"C:\Users\Public\Desktop\KINSEAT HÔTE.LNK"
        ]);
        var native = SeatShortcutPolicy.SelectShortcuts([
            Path.Combine("Desktop", "Kinseat Hôte.lnk"),
            Path.Combine("Desktop", "Kinseat HÃ´te.lnk"),
            Path.Combine("Programs", "HidHide", "HidHide Configurator.lnk"),
            Path.Combine("Programs", "HidHide", "HidHide Configuration Client.lnk"),
            Path.Combine("Desktop", "Jeu.lnk"),
            Path.Combine("Desktop", "Kinseat Hôte Custom.lnk"),
            Path.Combine("Desktop", "HidHide Extra.lnk"),
            Path.Combine("Desktop", "KINSEAT HÔTE.LNK")
        ]);
        Assert.Equal(4, native.Length);
        Assert.All(native, name => Assert.DoesNotContain("Jeu", name));
        Assert.All(native, name => Assert.DoesNotContain("Custom", name));
        Assert.All(native, name => Assert.DoesNotContain("Extra", name));
        if (OperatingSystem.IsWindows()) Assert.Equal(4, selected.Length);
    }

    [Fact]
    public void ShortcutRelocationIsIdempotentIncludesSubdirsAndRestoresCleanly()
    {
        using var sandbox = new TempShortcutSandbox();
        var acl = new FakeShortcutAclHandler();
        var logger = new CapturingLogger();

        var kinseat = Path.Combine(sandbox.Desktop, "Kinseat Hôte.lnk");
        var mojibake = Path.Combine(sandbox.Desktop, "Kinseat HÃ´te.lnk");
        var hidHideDir = Path.Combine(sandbox.Programs, "Nefarius", "HidHide");
        Directory.CreateDirectory(hidHideDir);
        var hidConfig = Path.Combine(hidHideDir, "HidHide Configurator.lnk");
        var hidClient = Path.Combine(hidHideDir, "HidHide Configuration Client.lnk");
        var game = Path.Combine(sandbox.Desktop, "Jeu.lnk");
        var prefixed = Path.Combine(sandbox.Desktop, "Kinseat Hôte Perso.lnk");

        File.WriteAllText(kinseat, "kinseat-lnk");
        File.WriteAllText(mojibake, "mojibake-lnk");
        File.WriteAllText(hidConfig, "hid-config-lnk");
        File.WriteAllText(hidClient, "hid-client-lnk");
        File.WriteAllText(game, "game-lnk");
        File.WriteAllText(prefixed, "perso-lnk");

        SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);

        Assert.False(File.Exists(kinseat));
        Assert.False(File.Exists(mojibake));
        Assert.False(File.Exists(hidConfig));
        Assert.False(File.Exists(hidClient));
        Assert.True(File.Exists(game));
        Assert.True(File.Exists(prefixed));
        Assert.True(File.Exists(sandbox.ManifestPath));

        var manifest = JsonSerializer.Deserialize<SeatShortcutPolicy.ShortcutManifest>(File.ReadAllText(sandbox.ManifestPath));
        Assert.NotNull(manifest);
        Assert.Equal(SeatShortcutPolicy.ManifestVersion, manifest!.Version);
        Assert.Equal(4, manifest.Entries.Count);
        Assert.All(manifest.Entries, e =>
        {
            Assert.Equal(SeatShortcutPolicy.StateRelocated, e.State);
            Assert.True(File.Exists(e.BackupPath));
            Assert.Equal(acl.CapturedSddl, e.OriginalSddl);
        });
        Assert.Contains(sandbox.BackupDir, acl.RestrictedDirectories);
        Assert.Contains(sandbox.ManifestPath, acl.RestrictedFiles);

        // Idempotent second run with active seats.
        SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);
        var manifestAfterSecond = JsonSerializer.Deserialize<SeatShortcutPolicy.ShortcutManifest>(File.ReadAllText(sandbox.ManifestPath));
        Assert.Equal(4, manifestAfterSecond!.Entries.Count);
        Assert.False(File.Exists(kinseat));

        // Restore when no managed seats remain.
        SeatShortcutPolicy.ApplyCore(sandbox.Paths, false, [], logger, acl);
        Assert.Equal("kinseat-lnk", File.ReadAllText(kinseat));
        Assert.Equal("mojibake-lnk", File.ReadAllText(mojibake));
        Assert.Equal("hid-config-lnk", File.ReadAllText(hidConfig));
        Assert.Equal("hid-client-lnk", File.ReadAllText(hidClient));
        Assert.False(File.Exists(sandbox.ManifestPath));
        Assert.Empty(Directory.GetFiles(sandbox.BackupDir));
        Assert.Equal(4, acl.RestoredAcls.Count);
        Assert.All(acl.RestoredAcls, r => Assert.Equal(acl.CapturedSddl, r.Sddl));
    }

    [Fact]
    public void ShortcutMigrationFromV1RemovesLegacyDenyAcesBeforeRelocating()
    {
        using var sandbox = new TempShortcutSandbox();
        var acl = new FakeShortcutAclHandler();
        var logger = new CapturingLogger();
        var kinseat = Path.Combine(sandbox.Desktop, "Kinseat Hôte.lnk");
        File.WriteAllText(kinseat, "kinseat-v1");

        Directory.CreateDirectory(sandbox.Store);
        var legacy = new[] { new SeatShortcutPolicy.LegacyShortcutEntry(kinseat, Seat) };
        File.WriteAllText(sandbox.ManifestPath, JsonSerializer.Serialize(legacy));

        SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);

        Assert.Contains((kinseat, Seat), acl.RemovedLegacyDenyAces);
        Assert.Contains(acl.StrippedBeforeMove, x => x.Path == kinseat && x.Sids.Contains(Seat));
        Assert.False(File.Exists(kinseat));
        var manifest = JsonSerializer.Deserialize<SeatShortcutPolicy.ShortcutManifest>(File.ReadAllText(sandbox.ManifestPath));
        Assert.NotNull(manifest);
        Assert.Equal(SeatShortcutPolicy.ManifestVersion, manifest!.Version);
        Assert.Single(manifest.Entries);
        Assert.Equal(SeatShortcutPolicy.StateRelocated, manifest.Entries[0].State);
    }

    [Fact]
    public void ShortcutCrashRecoveryHandlesPendingMoveAndPendingRestore()
    {
        using var sandbox = new TempShortcutSandbox();
        var acl = new FakeShortcutAclHandler();
        var logger = new CapturingLogger();
        Directory.CreateDirectory(sandbox.BackupDir);

        // 1) Crash after recording PendingMove, before File.Move happened.
        var orig1 = Path.Combine(sandbox.Desktop, "Kinseat Hôte.lnk");
        var bak1 = SeatShortcutPolicy.BuildBackupPath(sandbox.BackupDir, orig1);
        File.WriteAllText(orig1, "pending-before-move");
        var manifest1 = new SeatShortcutPolicy.ShortcutManifest(SeatShortcutPolicy.ManifestVersion, [
            new SeatShortcutPolicy.RelocatedShortcutEntry(orig1, bak1, SeatShortcutPolicy.StatePendingMove, acl.CapturedSddl)
        ]);
        File.WriteAllText(sandbox.ManifestPath, JsonSerializer.Serialize(manifest1));

        SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);
        Assert.False(File.Exists(orig1));
        Assert.Equal("pending-before-move", File.ReadAllText(bak1));
        var after1 = JsonSerializer.Deserialize<SeatShortcutPolicy.ShortcutManifest>(File.ReadAllText(sandbox.ManifestPath));
        Assert.Equal(SeatShortcutPolicy.StateRelocated, after1!.Entries[0].State);

        // 2) Crash after File.Move happened during PendingMove, before manifest updated.
        var manifest2 = new SeatShortcutPolicy.ShortcutManifest(SeatShortcutPolicy.ManifestVersion, [
            new SeatShortcutPolicy.RelocatedShortcutEntry(orig1, bak1, SeatShortcutPolicy.StatePendingMove, acl.CapturedSddl)
        ]);
        File.WriteAllText(sandbox.ManifestPath, JsonSerializer.Serialize(manifest2));
        SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);
        var after2 = JsonSerializer.Deserialize<SeatShortcutPolicy.ShortcutManifest>(File.ReadAllText(sandbox.ManifestPath));
        Assert.Equal(SeatShortcutPolicy.StateRelocated, after2!.Entries[0].State);

        // 3) Crash after File.Move during PendingRestore (file already back in place, backup gone).
        File.Move(bak1, orig1);
        var manifest3 = new SeatShortcutPolicy.ShortcutManifest(SeatShortcutPolicy.ManifestVersion, [
            new SeatShortcutPolicy.RelocatedShortcutEntry(orig1, bak1, SeatShortcutPolicy.StatePendingRestore, acl.CapturedSddl)
        ]);
        File.WriteAllText(sandbox.ManifestPath, JsonSerializer.Serialize(manifest3));
        SeatShortcutPolicy.ApplyCore(sandbox.Paths, false, [], logger, acl);
        Assert.True(File.Exists(orig1));
        Assert.False(File.Exists(sandbox.ManifestPath));
        Assert.Contains(acl.RestoredAcls, r => r.Path == orig1 && r.Sddl == acl.CapturedSddl);
    }

    [Fact]
    public void ShortcutRestoreLogsConflictWithoutOverwritingRecreatedPublicFile()
    {
        using var sandbox = new TempShortcutSandbox();
        var acl = new FakeShortcutAclHandler();
        var logger = new CapturingLogger();

        var kinseat = Path.Combine(sandbox.Desktop, "Kinseat Hôte.lnk");
        var hidClient = Path.Combine(sandbox.Programs, "HidHide Configuration Client.lnk");
        File.WriteAllText(kinseat, "original-kinseat");
        File.WriteAllText(hidClient, "original-hid");

        SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);
        Assert.False(File.Exists(kinseat));
        Assert.False(File.Exists(hidClient));

        // Recreate public shortcut while seats were active (e.g. reinstall).
        File.WriteAllText(kinseat, "recreated-kinseat");

        SeatShortcutPolicy.ApplyCore(sandbox.Paths, false, [], logger, acl);

        // Recreated file must NOT be overwritten; other shortcut MUST still restore.
        Assert.Equal("recreated-kinseat", File.ReadAllText(kinseat));
        Assert.Equal("original-hid", File.ReadAllText(hidClient));
        Assert.Contains(logger.Entries, e => e.Level == LogLevel.Warning && e.Message.Contains("Conflit"));
        var remaining = JsonSerializer.Deserialize<SeatShortcutPolicy.ShortcutManifest>(File.ReadAllText(sandbox.ManifestPath));
        Assert.Single(remaining!.Entries);
        Assert.Equal(SeatShortcutPolicy.StateRestoreConflict, remaining.Entries[0].State);
        Assert.Equal("original-kinseat", File.ReadAllText(remaining.Entries[0].BackupPath));
    }

    [Fact]
    public void ShortcutPolicyRejectsOutOfScopePathsAndSymlinks()
    {
        using var sandbox = new TempShortcutSandbox();
        var acl = new FakeShortcutAclHandler();
        var logger = new CapturingLogger();
        Directory.CreateDirectory(sandbox.BackupDir);

        var outsideTarget = Path.Combine(sandbox.Outside, "Kinseat Hôte.lnk");
        var outsideBackup = Path.Combine(sandbox.Outside, "escaped.lnk");
        var traversalOrig = Path.Combine(sandbox.Desktop, "..", "Outside", "Kinseat Hôte.lnk");
        var traversalBak = Path.Combine(sandbox.BackupDir, "..", "..", "Outside", "escaped.lnk");
        var gamePath = Path.Combine(sandbox.Desktop, "Jeu.lnk");
        var validBak = Path.Combine(sandbox.BackupDir, "aaaa_Jeu.lnk");

        File.WriteAllText(outsideTarget, "outside");
        File.WriteAllText(outsideBackup, "outside-bak");
        File.WriteAllText(validBak, "bak-game");

        var crafted = new SeatShortcutPolicy.ShortcutManifest(SeatShortcutPolicy.ManifestVersion, [
            new SeatShortcutPolicy.RelocatedShortcutEntry(traversalOrig, validBak, SeatShortcutPolicy.StateRelocated),
            new SeatShortcutPolicy.RelocatedShortcutEntry(Path.Combine(sandbox.Desktop, "Kinseat Hôte.lnk"), traversalBak, SeatShortcutPolicy.StateRelocated),
            new SeatShortcutPolicy.RelocatedShortcutEntry(gamePath, validBak, SeatShortcutPolicy.StateRelocated)
        ]);
        File.WriteAllText(sandbox.ManifestPath, JsonSerializer.Serialize(crafted));

        SeatShortcutPolicy.ApplyCore(sandbox.Paths, false, [], logger, acl);

        Assert.False(File.Exists(gamePath));
        Assert.Equal("outside", File.ReadAllText(outsideTarget));
        Assert.Equal("outside-bak", File.ReadAllText(outsideBackup));
        Assert.Equal("bak-game", File.ReadAllText(validBak));
        Assert.True(logger.Entries.Count(e => e.Level == LogLevel.Warning) >= 3);

        // Symlinked directory and symlinked .lnk file must never be followed or relocated.
        var symDir = Path.Combine(sandbox.Programs, "SymLinkDir");
        var symFile = Path.Combine(sandbox.Desktop, "HidHide Configuration Client.lnk");
        Directory.CreateSymbolicLink(symDir, sandbox.Outside);
        File.CreateSymbolicLink(symFile, outsideTarget);

        File.Delete(sandbox.ManifestPath);
        SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);

        Assert.True(File.Exists(outsideTarget));
        Assert.False(File.Exists(sandbox.ManifestPath));
        Assert.Empty(SeatShortcutPolicy.DiscoverTargetShortcuts(sandbox.Paths.PublicRoots, logger));
    }

    [Fact]
    public void ShortcutPendingMoveConflictPreservesBothFilesWithoutOverwrite()
    {
        // 1) Active seats (ReconcileExistingEntries): PendingMove with both original and backup must keep both.
        using (var sandbox = new TempShortcutSandbox())
        {
            var acl = new FakeShortcutAclHandler();
            var logger = new CapturingLogger();
            Directory.CreateDirectory(sandbox.BackupDir);

            var orig = Path.Combine(sandbox.Desktop, "Kinseat Hôte.lnk");
            var bak = SeatShortcutPolicy.BuildBackupPath(sandbox.BackupDir, orig);
            File.WriteAllText(orig, "original-newer");
            File.WriteAllText(bak, "backup-older");

            var manifest = new SeatShortcutPolicy.ShortcutManifest(SeatShortcutPolicy.ManifestVersion, [
                new SeatShortcutPolicy.RelocatedShortcutEntry(orig, bak, SeatShortcutPolicy.StatePendingMove, acl.CapturedSddl)
            ]);
            File.WriteAllText(sandbox.ManifestPath, JsonSerializer.Serialize(manifest));

            SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);

            Assert.Equal("original-newer", File.ReadAllText(orig));
            Assert.Equal("backup-older", File.ReadAllText(bak));
            var updated = JsonSerializer.Deserialize<SeatShortcutPolicy.ShortcutManifest>(File.ReadAllText(sandbox.ManifestPath));
            Assert.Single(updated!.Entries);
            Assert.Equal(SeatShortcutPolicy.StateRestoreConflict, updated.Entries[0].State);
            Assert.Contains(logger.Entries, e => e.Level == LogLevel.Warning && e.Message.Contains("Conflit"));
        }

        // 2) No managed seats (RestoreAll): PendingMove with both original and backup must keep both.
        using (var sandbox = new TempShortcutSandbox())
        {
            var acl = new FakeShortcutAclHandler();
            var logger = new CapturingLogger();
            Directory.CreateDirectory(sandbox.BackupDir);

            var orig = Path.Combine(sandbox.Desktop, "HidHide Configurator.lnk");
            var bak = SeatShortcutPolicy.BuildBackupPath(sandbox.BackupDir, orig);
            File.WriteAllText(orig, "original-restore");
            File.WriteAllText(bak, "backup-restore");

            var manifest = new SeatShortcutPolicy.ShortcutManifest(SeatShortcutPolicy.ManifestVersion, [
                new SeatShortcutPolicy.RelocatedShortcutEntry(orig, bak, SeatShortcutPolicy.StatePendingMove, acl.CapturedSddl)
            ]);
            File.WriteAllText(sandbox.ManifestPath, JsonSerializer.Serialize(manifest));

            SeatShortcutPolicy.ApplyCore(sandbox.Paths, false, [], logger, acl);

            Assert.Equal("original-restore", File.ReadAllText(orig));
            Assert.Equal("backup-restore", File.ReadAllText(bak));
            var updated = JsonSerializer.Deserialize<SeatShortcutPolicy.ShortcutManifest>(File.ReadAllText(sandbox.ManifestPath));
            Assert.Single(updated!.Entries);
            Assert.Equal(SeatShortcutPolicy.StateRestoreConflict, updated.Entries[0].State);
            Assert.Contains(logger.Entries, e => e.Level == LogLevel.Warning && e.Message.Contains("Conflit"));
        }
    }

    [Fact]
    public void ShortcutNormalRelocationNeverOverwritesExistingBackup()
    {
        // Case A: Existing Relocated entry + recreated public file -> no overwrite, RestoreConflict.
        using (var sandbox = new TempShortcutSandbox())
        {
            var acl = new FakeShortcutAclHandler();
            var logger = new CapturingLogger();
            var orig = Path.Combine(sandbox.Desktop, "Kinseat Hôte.lnk");
            File.WriteAllText(orig, "original-v1");

            SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);
            var bak = SeatShortcutPolicy.BuildBackupPath(sandbox.BackupDir, orig);
            Assert.False(File.Exists(orig));
            Assert.Equal("original-v1", File.ReadAllText(bak));

            // Recreate public shortcut while backup already exists.
            File.WriteAllText(orig, "recreated-v2");
            SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);

            Assert.Equal("recreated-v2", File.ReadAllText(orig));
            Assert.Equal("original-v1", File.ReadAllText(bak));
            var manifest = JsonSerializer.Deserialize<SeatShortcutPolicy.ShortcutManifest>(File.ReadAllText(sandbox.ManifestPath));
            Assert.Single(manifest!.Entries);
            Assert.Equal(SeatShortcutPolicy.StateRestoreConflict, manifest.Entries[0].State);
            Assert.Contains(logger.Entries, e => e.Level == LogLevel.Warning && e.Message.Contains("Conflit"));
        }

        // Case B: No manifest entry, orphan backup already exists at deterministic path -> no overwrite, RestoreConflict.
        using (var sandbox = new TempShortcutSandbox())
        {
            var acl = new FakeShortcutAclHandler();
            var logger = new CapturingLogger();
            Directory.CreateDirectory(sandbox.BackupDir);
            var orig = Path.Combine(sandbox.Desktop, "HidHide Configuration Client.lnk");
            var bak = SeatShortcutPolicy.BuildBackupPath(sandbox.BackupDir, orig);
            File.WriteAllText(orig, "public-file");
            File.WriteAllText(bak, "orphan-backup");

            SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);

            Assert.Equal("public-file", File.ReadAllText(orig));
            Assert.Equal("orphan-backup", File.ReadAllText(bak));
            var manifest = JsonSerializer.Deserialize<SeatShortcutPolicy.ShortcutManifest>(File.ReadAllText(sandbox.ManifestPath));
            Assert.Single(manifest!.Entries);
            Assert.Equal(SeatShortcutPolicy.StateRestoreConflict, manifest.Entries[0].State);
            Assert.Contains(logger.Entries, e => e.Level == LogLevel.Warning && e.Message.Contains("Conflit"));
        }
    }

    [Fact]
    public void ShortcutAclRestrictionFailureRollsBackToOriginalAndPreservesState()
    {
        using var sandbox = new TempShortcutSandbox();
        var acl = new FakeShortcutAclHandler
        {
            FailRestrictPredicate = p => p.StartsWith(sandbox.BackupDir + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase)
        };
        var logger = new CapturingLogger();

        var orig = Path.Combine(sandbox.Desktop, "Kinseat Hôte.lnk");
        var expectedBak = SeatShortcutPolicy.BuildBackupPath(sandbox.BackupDir, orig);
        File.WriteAllText(orig, "kinseat-acl-rollback");

        SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);

        // ACL restriction was attempted on a temp file inside BackupDir before renaming to final backup name.
        Assert.Contains(acl.RestrictedFiles, p =>
            p.StartsWith(sandbox.BackupDir + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase) &&
            p.EndsWith(".tmp", StringComparison.Ordinal) &&
            !string.Equals(p, expectedBak, StringComparison.OrdinalIgnoreCase));
        Assert.DoesNotContain(expectedBak, acl.RestrictedFiles);

        // Rollback restored the original file, left no final backup or temp file, and preserved PendingMove state.
        Assert.Equal("kinseat-acl-rollback", File.ReadAllText(orig));
        Assert.False(File.Exists(expectedBak));
        Assert.Empty(Directory.GetFiles(sandbox.BackupDir));
        var manifest = JsonSerializer.Deserialize<SeatShortcutPolicy.ShortcutManifest>(File.ReadAllText(sandbox.ManifestPath));
        Assert.Single(manifest!.Entries);
        Assert.Equal(SeatShortcutPolicy.StatePendingMove, manifest.Entries[0].State);
        Assert.Contains(logger.Entries, e => e.Level == LogLevel.Warning);

        // Subsequent run once ACL restriction succeeds completes PendingMove cleanly.
        acl.FailRestrictPredicate = null;
        SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);
        Assert.False(File.Exists(orig));
        Assert.Equal("kinseat-acl-rollback", File.ReadAllText(expectedBak));
        var after = JsonSerializer.Deserialize<SeatShortcutPolicy.ShortcutManifest>(File.ReadAllText(sandbox.ManifestPath));
        Assert.Single(after!.Entries);
        Assert.Equal(SeatShortcutPolicy.StateRelocated, after.Entries[0].State);
    }

    [Fact]
    public void ShortcutManifestWithMismatchedBackupHashIsRejectedWithoutMutation()
    {
        using var sandbox = new TempShortcutSandbox();
        var acl = new FakeShortcutAclHandler();
        var logger = new CapturingLogger();
        Directory.CreateDirectory(sandbox.BackupDir);

        var orig = Path.Combine(sandbox.Desktop, "Kinseat Hôte.lnk");
        var otherOrig = Path.Combine(sandbox.Programs, "Kinseat Hôte.lnk");
        var validBak = SeatShortcutPolicy.BuildBackupPath(sandbox.BackupDir, orig);
        var otherHashBak = SeatShortcutPolicy.BuildBackupPath(sandbox.BackupDir, otherOrig);
        var bogusHashBak = Path.Combine(sandbox.BackupDir, "0000000000000000_Kinseat Hôte.lnk");

        Assert.True(SeatShortcutPolicy.ValidateBackupPath(sandbox.BackupDir, orig, validBak, out _));
        Assert.False(SeatShortcutPolicy.ValidateBackupPath(sandbox.BackupDir, orig, otherHashBak, out _));
        Assert.False(SeatShortcutPolicy.ValidateBackupPath(sandbox.BackupDir, orig, bogusHashBak, out _));
        Assert.False(SeatShortcutPolicy.ValidateBackupPath(sandbox.BackupDir, orig, Path.Combine(sandbox.BackupDir, "1111_Kinseat Hôte.lnk"), out _));

        File.WriteAllText(orig, "orig-untouched");
        File.WriteAllText(otherHashBak, "other-hash-backup");

        var crafted = new SeatShortcutPolicy.ShortcutManifest(SeatShortcutPolicy.ManifestVersion, [
            new SeatShortcutPolicy.RelocatedShortcutEntry(orig, otherHashBak, SeatShortcutPolicy.StatePendingMove, acl.CapturedSddl)
        ]);
        var craftedJson = JsonSerializer.Serialize(crafted);
        File.WriteAllText(sandbox.ManifestPath, craftedJson);

        // Active seats: rejected without mutating original, backup, or manifest.
        SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);
        Assert.Equal("orig-untouched", File.ReadAllText(orig));
        Assert.Equal("other-hash-backup", File.ReadAllText(otherHashBak));
        Assert.False(File.Exists(validBak));
        Assert.Equal(craftedJson, File.ReadAllText(sandbox.ManifestPath));
        Assert.Contains(logger.Entries, e => e.Level == LogLevel.Warning);

        // Restore pass: also rejected without mutating original, backup, or manifest.
        logger.Entries.Clear();
        SeatShortcutPolicy.ApplyCore(sandbox.Paths, false, [], logger, acl);
        Assert.Equal("orig-untouched", File.ReadAllText(orig));
        Assert.Equal("other-hash-backup", File.ReadAllText(otherHashBak));
        Assert.Equal(craftedJson, File.ReadAllText(sandbox.ManifestPath));
        Assert.Contains(logger.Entries, e => e.Level == LogLevel.Warning);
    }

    [Fact]
    public void ShortcutPolicyDefaultRootsIncludeCommonStartMenuProgramsAndDesktop()
    {
        Assert.Contains(Environment.SpecialFolder.CommonDesktopDirectory, SeatShortcutPolicy.DefaultPublicRootFolders);
        Assert.Contains(Environment.SpecialFolder.CommonStartMenu, SeatShortcutPolicy.DefaultPublicRootFolders);
        Assert.Contains(Environment.SpecialFolder.CommonPrograms, SeatShortcutPolicy.DefaultPublicRootFolders);
    }

    [Fact]
    public void ShortcutRelocationCoversStartMenuRootAndNestedProgramsWithoutDuplicateTraversal()
    {
        using var sandbox = new TempShortcutSandbox();
        var acl = new FakeShortcutAclHandler();
        var logger = new CapturingLogger();

        var rootHidClient = Path.Combine(sandbox.StartMenu, "HidHide Configuration Client.lnk");
        var rootKinseat = Path.Combine(sandbox.StartMenu, "Kinseat Hôte.lnk");
        var rootOther = Path.Combine(sandbox.StartMenu, "AutreOutil.lnk");
        var rootNearMatch = Path.Combine(sandbox.StartMenu, "HidHide.lnk");

        var nestedDir = Path.Combine(sandbox.Programs, "Nefarius", "HidHide");
        Directory.CreateDirectory(nestedDir);
        var nestedHidClient = Path.Combine(nestedDir, "HidHide Configuration Client.lnk");
        var nestedHidConfig = Path.Combine(nestedDir, "HidHide Configurator.lnk");
        var nestedGame = Path.Combine(sandbox.Programs, "Jeu.lnk");

        File.WriteAllText(rootHidClient, "startmenu-root-hid-client");
        File.WriteAllText(rootKinseat, "startmenu-root-kinseat");
        File.WriteAllText(rootOther, "startmenu-other-tool");
        File.WriteAllText(rootNearMatch, "startmenu-near-match");
        File.WriteAllText(nestedHidClient, "programs-nested-hid-client");
        File.WriteAllText(nestedHidConfig, "programs-nested-hid-config");
        File.WriteAllText(nestedGame, "programs-game");

        // Duplicate + reversed roots (Programs nested under StartMenu): single traversal and no duplicates.
        var symNested = Path.Combine(sandbox.Programs, "SymNested");
        Directory.CreateSymbolicLink(symNested, sandbox.Outside);
        var discoveryLogger = new CapturingLogger();
        var discovered = SeatShortcutPolicy.DiscoverTargetShortcuts(
            [sandbox.Programs, sandbox.StartMenu, sandbox.Programs, sandbox.Desktop],
            discoveryLogger);
        Directory.Delete(symNested);

        Assert.Equal(4, discovered.Length);
        Assert.Contains(rootHidClient, discovered);
        Assert.Contains(rootKinseat, discovered);
        Assert.Contains(nestedHidClient, discovered);
        Assert.Contains(nestedHidConfig, discovered);
        Assert.DoesNotContain(rootOther, discovered);
        Assert.DoesNotContain(rootNearMatch, discovered);
        Assert.DoesNotContain(nestedGame, discovered);
        Assert.Single(discoveryLogger.Entries, e => e.Level == LogLevel.Warning && e.Message.Contains("SymNested"));

        // Relocate with active managed seat.
        SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);

        Assert.False(File.Exists(rootHidClient));
        Assert.False(File.Exists(rootKinseat));
        Assert.False(File.Exists(nestedHidClient));
        Assert.False(File.Exists(nestedHidConfig));
        Assert.Equal("startmenu-other-tool", File.ReadAllText(rootOther));
        Assert.Equal("startmenu-near-match", File.ReadAllText(rootNearMatch));
        Assert.Equal("programs-game", File.ReadAllText(nestedGame));

        var manifest = JsonSerializer.Deserialize<SeatShortcutPolicy.ShortcutManifest>(File.ReadAllText(sandbox.ManifestPath));
        Assert.NotNull(manifest);
        Assert.Equal(SeatShortcutPolicy.ManifestVersion, manifest!.Version);
        Assert.Equal(4, manifest.Entries.Count);
        Assert.Equal(4, manifest.Entries.Select(e => e.BackupPath).Distinct(StringComparer.OrdinalIgnoreCase).Count());
        Assert.All(manifest.Entries, e =>
        {
            Assert.Equal(SeatShortcutPolicy.StateRelocated, e.State);
            Assert.True(File.Exists(e.BackupPath));
            Assert.Equal(acl.CapturedSddl, e.OriginalSddl);
        });

        // Simulated service restart while managed seat is still active: idempotent.
        SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);
        var manifestAfterRestart = JsonSerializer.Deserialize<SeatShortcutPolicy.ShortcutManifest>(File.ReadAllText(sandbox.ManifestPath));
        Assert.Equal(4, manifestAfterRestart!.Entries.Count);
        Assert.False(File.Exists(rootHidClient));
        Assert.False(File.Exists(nestedHidClient));

        // Automatic restoration when no managed seat remains: exact files restored without confusion.
        SeatShortcutPolicy.ApplyCore(sandbox.Paths, false, [], logger, acl);
        Assert.Equal("startmenu-root-hid-client", File.ReadAllText(rootHidClient));
        Assert.Equal("startmenu-root-kinseat", File.ReadAllText(rootKinseat));
        Assert.Equal("programs-nested-hid-client", File.ReadAllText(nestedHidClient));
        Assert.Equal("programs-nested-hid-config", File.ReadAllText(nestedHidConfig));
        Assert.Equal("startmenu-other-tool", File.ReadAllText(rootOther));
        Assert.Equal("startmenu-near-match", File.ReadAllText(rootNearMatch));
        Assert.Equal("programs-game", File.ReadAllText(nestedGame));
        Assert.False(File.Exists(sandbox.ManifestPath));
        Assert.Empty(Directory.GetFiles(sandbox.BackupDir));
        Assert.Equal(4, acl.RestoredAcls.Count);
    }

    [Fact]
    public void ShortcutPolicyRejectsSymlinkedStartMenuRootEvenWhenProgramsIsAlsoListed()
    {
        using var sandbox = new TempShortcutSandbox();
        var acl = new FakeShortcutAclHandler();
        var logger = new CapturingLogger();

        var realStartMenu = Path.Combine(sandbox.Outside, "RealStartMenu");
        var realPrograms = Path.Combine(realStartMenu, "Programs");
        Directory.CreateDirectory(realPrograms);
        var realRootShortcut = Path.Combine(realStartMenu, "HidHide Configuration Client.lnk");
        var realNestedShortcut = Path.Combine(realPrograms, "HidHide Configurator.lnk");
        File.WriteAllText(realRootShortcut, "outside-root");
        File.WriteAllText(realNestedShortcut, "outside-nested");

        var linkedStartMenu = Path.Combine(sandbox.Root, "LinkedStartMenu");
        Directory.CreateSymbolicLink(linkedStartMenu, realStartMenu);
        var linkedPrograms = Path.Combine(linkedStartMenu, "Programs");
        var paths = new SeatShortcutPolicy.PolicyPaths(
            [linkedPrograms, linkedStartMenu],
            sandbox.ManifestPath,
            sandbox.BackupDir);

        Assert.Empty(SeatShortcutPolicy.DiscoverTargetShortcuts(paths.PublicRoots, logger));
        SeatShortcutPolicy.ApplyCore(paths, true, [Seat], logger, acl);

        Assert.Equal("outside-root", File.ReadAllText(realRootShortcut));
        Assert.Equal("outside-nested", File.ReadAllText(realNestedShortcut));
        Assert.False(File.Exists(sandbox.ManifestPath));
        Assert.Contains(logger.Entries, e => e.Level == LogLevel.Warning);
    }

    [Fact]
    public void ShortcutPolicyLogsIgnoredReparsePointsAtMostOnceAndDowngradesKnownCompatJunction()
    {
        using var sandbox = new TempShortcutSandbox();
        var acl = new FakeShortcutAclHandler();
        var logger = new CapturingLogger();

        // Known Windows compatibility junction pattern: Start Menu\Programmes -> Start Menu\Programs.
        var compatJunction = Path.Combine(sandbox.StartMenu, "Programmes");
        Directory.CreateSymbolicLink(compatJunction, sandbox.Programs);

        // Unexpected reparse point targeting outside the public root.
        var unexpectedLink = Path.Combine(sandbox.Programs, "UnexpectedLink");
        Directory.CreateSymbolicLink(unexpectedLink, sandbox.Outside);
        var outsideShortcut = Path.Combine(sandbox.Outside, "HidHide Configurator.lnk");
        File.WriteAllText(outsideShortcut, "outside-untouched");

        // Two reconciliation passes: neither reparse point is followed, and each path is logged at most once.
        SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);
        SeatShortcutPolicy.ApplyCore(sandbox.Paths, true, [Seat], logger, acl);

        Assert.Equal("outside-untouched", File.ReadAllText(outsideShortcut));
        Assert.False(File.Exists(sandbox.ManifestPath));

        Assert.Single(logger.Entries, e => e.Level == LogLevel.Debug && e.Message.Contains("Programmes"));
        Assert.DoesNotContain(logger.Entries, e => e.Level == LogLevel.Warning && e.Message.Contains("Programmes"));
        Assert.Single(logger.Entries, e => e.Level == LogLevel.Warning && e.Message.Contains("UnexpectedLink"));

        // Hidden+System attributes also classify a junction as a known Windows compatibility junction.
        Assert.True(SeatShortcutPolicy.IsKnownWindowsCompatibilityJunction(
            sandbox.StartMenu,
            unexpectedLink,
            FileAttributes.Directory | FileAttributes.Hidden | FileAttributes.System | FileAttributes.ReparsePoint));
        Assert.False(SeatShortcutPolicy.IsKnownWindowsCompatibilityJunction(
            sandbox.StartMenu,
            unexpectedLink,
            FileAttributes.Directory | FileAttributes.ReparsePoint));
    }

    [Fact]
    public void ShortcutPolicyClassifiesIgnoredReparseSubdirectoryAtMostOnceAcrossPasses()
    {
        using var sandbox = new TempShortcutSandbox();
        var logger = new CapturingLogger();
        var logState = new SeatShortcutPolicy.ReparsePointLogState();

        var compatJunction = Path.Combine(sandbox.StartMenu, "Programmes");
        Directory.CreateSymbolicLink(compatJunction, sandbox.Programs);

        var unexpectedLink = Path.Combine(sandbox.Programs, "UnexpectedLink");
        Directory.CreateSymbolicLink(unexpectedLink, sandbox.Outside);
        File.WriteAllText(Path.Combine(sandbox.Outside, "HidHide Configurator.lnk"), "outside-untouched");

        var classificationCalls = new Dictionary<string, int>(StringComparer.Ordinal);
        bool Classifier(string root, string subdir)
        {
            classificationCalls[subdir] = classificationCalls.GetValueOrDefault(subdir) + 1;
            return SeatShortcutPolicy.IsKnownWindowsCompatibilityJunction(root, subdir);
        }

        for (var i = 0; i < 3; i++)
            Assert.Empty(SeatShortcutPolicy.DiscoverTargetShortcuts(sandbox.Paths.PublicRoots, logger, Classifier, logState));

        Assert.Equal(2, classificationCalls.Count);
        Assert.All(classificationCalls.Values, count => Assert.Equal(1, count));
        Assert.Single(logger.Entries, e => e.Level == LogLevel.Debug && e.Message.Contains("Programmes"));
        Assert.Single(logger.Entries, e => e.Level == LogLevel.Warning && e.Message.Contains("UnexpectedLink"));
    }

    [Fact]
    public void ShortcutPolicyBoundsReparsePointLoggingAndWarnsOnceWhenLimitReached()
    {
        using var sandbox = new TempShortcutSandbox();
        var logger = new CapturingLogger();
        var logState = new SeatShortcutPolicy.ReparsePointLogState(maxEntries: 2);

        File.WriteAllText(Path.Combine(sandbox.Outside, "HidHide Configurator.lnk"), "outside-untouched");
        for (var i = 0; i < 4; i++)
            Directory.CreateSymbolicLink(Path.Combine(sandbox.Programs, $"Link{i}"), sandbox.Outside);

        var classificationCalls = 0;
        bool Classifier(string root, string subdir)
        {
            classificationCalls++;
            return SeatShortcutPolicy.IsKnownWindowsCompatibilityJunction(root, subdir);
        }

        Assert.Empty(SeatShortcutPolicy.DiscoverTargetShortcuts(sandbox.Paths.PublicRoots, logger, Classifier, logState));
        Assert.Empty(SeatShortcutPolicy.DiscoverTargetShortcuts(sandbox.Paths.PublicRoots, logger, Classifier, logState));

        Assert.Equal(256, SeatShortcutPolicy.MaxLoggedReparsePaths);
        Assert.Equal(2, logState.Count);
        Assert.Equal(2, classificationCalls);
        Assert.Equal(2, logger.Entries.Count(e => e.Level == LogLevel.Warning && e.Message.Contains("Sous-répertoire public ignoré")));
        Assert.Single(logger.Entries, e => e.Level == LogLevel.Warning && e.Message.Contains("Limite de journalisation des reparse points atteinte"));
    }
}
