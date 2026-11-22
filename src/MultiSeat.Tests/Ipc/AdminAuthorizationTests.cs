using System.IO.Pipes;
using System.Security.AccessControl;
using System.Security.Claims;
using System.Security.Principal;
using MultiSeat.Service.Ipc;
using Xunit;

namespace MultiSeat.Tests.Ipc;

public class AdminAuthorizationTests
{
    private const string Admin = "S-1-5-21-1111111111-2222222222-3333333333-1001";
    private const string Other = "S-1-5-21-1111111111-2222222222-3333333333-1002";

    private static string TempFile() =>
        Path.Combine(Path.GetTempPath(), "kinseat-auth-" + Guid.NewGuid().ToString("N"), "admin-authorizations.json");

    [Fact]
    public void Store_persists_is_idempotent_and_revokes()
    {
        var path = TempFile();
        try
        {
            var store = new AdminAuthorizationStore(path);
            Assert.False(store.Contains(Admin));
            Assert.True(store.Add(Admin, @"PC\camille", DateTimeOffset.UnixEpoch));
            Assert.False(store.Add(Admin, @"PC\autre", DateTimeOffset.UnixEpoch));
            Assert.Single(store.GetAll());

            var reloaded = new AdminAuthorizationStore(path);
            Assert.True(reloaded.Contains(Admin));
            Assert.False(reloaded.Contains(Other));
            Assert.Equal(@"PC\camille", reloaded.GetAll()[0].Name);

            Assert.True(reloaded.Remove(Admin));
            Assert.False(reloaded.Remove(Admin));
            Assert.False(new AdminAuthorizationStore(path).Contains(Admin));
        }
        finally { Directory.Delete(Path.GetDirectoryName(path)!, recursive: true); }
    }

    [Fact]
    public void Corrupt_or_forged_file_authorizes_nobody()
    {
        var path = TempFile();
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        try
        {
            File.WriteAllText(path, "{ pas du json");
            Assert.Empty(new AdminAuthorizationStore(path).GetAll());
            File.WriteAllText(path, """[{"sid":"pas-un-sid","name":"x","authorizedAt":"2026-01-01T00:00:00Z"}]""");
            Assert.Empty(new AdminAuthorizationStore(path).GetAll());
            Assert.False(new AdminAuthorizationStore(path).Contains(null));
        }
        finally { Directory.Delete(Path.GetDirectoryName(path)!, recursive: true); }
    }

    [Fact]
    public void Add_rejects_invalid_sid()
    {
        var path = TempFile();
        Assert.Throws<ArgumentException>(() => new AdminAuthorizationStore(path).Add("utilisateur", "x", DateTimeOffset.UnixEpoch));
        Assert.False(File.Exists(path));
    }

    [Theory]
    [InlineData(true, false, false, false, true)]   // administrateur élevé
    [InlineData(false, true, false, false, true)]   // SYSTEM
    [InlineData(false, false, true, true, true)]    // autorisé une fois, toujours administrateur
    [InlineData(false, false, true, false, false)]  // autorisé puis retiré des administrateurs
    [InlineData(false, false, false, true, false)]  // administrateur non élevé jamais autorisé
    [InlineData(false, false, false, false, false)] // utilisateur standard
    public void Access_rule(bool elevated, bool system, bool listed, bool member, bool expected) =>
        Assert.Equal(expected, AdminAuthorizationStore.IsAllowed(elevated, system, listed, member));

    [Fact]
    public void Administrators_membership_counts_deny_only_group()
    {
        Assert.True(AdminAuthorizationStore.IsAdministratorsMember([new Claim(ClaimTypes.DenyOnlySid, "S-1-5-32-544")]));
        Assert.True(AdminAuthorizationStore.IsAdministratorsMember([new Claim(ClaimTypes.GroupSid, "S-1-5-32-544")]));
        Assert.False(AdminAuthorizationStore.IsAdministratorsMember([new Claim(ClaimTypes.GroupSid, "S-1-5-32-545")]));
        Assert.False(AdminAuthorizationStore.IsAdministratorsMember([new Claim(ClaimTypes.Name, "S-1-5-32-544")]));
    }

    [Fact]
    public void Authorize_requires_elevation_and_refuses_system_and_seat_accounts()
    {
        string[] seats = ["ks-lea", "ks-tom"];
        Assert.Null(AdminAuthorizationStore.AuthorizeRefusal(true, Admin, "camille", seats));
        Assert.NotNull(AdminAuthorizationStore.AuthorizeRefusal(false, Admin, "camille", seats));
        Assert.NotNull(AdminAuthorizationStore.AuthorizeRefusal(true, "S-1-5-18", "SYSTEM", seats));
        Assert.NotNull(AdminAuthorizationStore.AuthorizeRefusal(true, null, "camille", seats));
        Assert.NotNull(AdminAuthorizationStore.AuthorizeRefusal(true, Admin, "KS-LEA", seats));
    }

    [Fact]
    public void Pipe_dacl_grants_listed_accounts_read_write_only()
    {
        var security = AdminPipeServer.CreateSecurity([Admin, Admin.ToLowerInvariant(), "invalide"]);
        var rules = security.GetAccessRules(true, false, typeof(SecurityIdentifier)).Cast<PipeAccessRule>().ToList();
        Assert.True(security.AreAccessRulesProtected);
        Assert.Contains(rules, r => r.AccessControlType == AccessControlType.Deny
            && ((SecurityIdentifier)r.IdentityReference).IsWellKnown(WellKnownSidType.NetworkSid));
        var granted = rules.Where(r => r.IdentityReference.Value == Admin).ToList();
        var rule = Assert.Single(granted);
        Assert.Equal(AccessControlType.Allow, rule.AccessControlType);
        Assert.Equal(0, (int)(rule.PipeAccessRights & (PipeAccessRights.ChangePermissions | PipeAccessRights.TakeOwnership)));
        Assert.Equal(PipeAccessRights.ReadWrite, rule.PipeAccessRights & PipeAccessRights.ReadWrite);
        Assert.Equal(4, rules.Count); // refus réseau, SYSTEM, Administrateurs, compte autorisé
    }

    [Fact]
    public async Task Dispatcher_accepts_authorization_methods_without_params()
    {
        foreach (var method in new[] { "app.authorization", "app.authorize", "app.revoke" })
        {
            var line = System.Text.Encoding.UTF8.GetBytes($$$"""{"id":7,"method":"{{{method}}}","params":{}}""");
            var reply = await AdminDispatcher.DispatchAsync(line, (m, _) => Task.FromResult<object?>(new { called = m }));
            var json = System.Text.Json.JsonSerializer.Serialize(reply);
            Assert.Contains(method, json);
            var extra = System.Text.Encoding.UTF8.GetBytes($$$"""{"id":7,"method":"{{{method}}}","params":{"sid":"S-1-5-18"}}""");
            Assert.Contains("invalid_params", System.Text.Json.JsonSerializer.Serialize(
                await AdminDispatcher.DispatchAsync(extra, (m, _) => Task.FromResult<object?>(null))));
        }
    }
}
