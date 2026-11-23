using System.Text;
using System.Text.Json;
using MultiSeat.Service.Accounts;
using MultiSeat.Service.Ipc;
using Xunit;

namespace MultiSeat.Tests.Ipc;

public sealed class Kinseat0311ServiceTests
{
    private const string TaskXml = "<Task xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\"><Triggers>{0}</Triggers></Task>";

    [Theory]
    [InlineData("HWiNFO", "powershell.exe", "overlay")]
    [InlineData("Start Something", "RTSS.exe", "overlay")]
    [InlineData("SUM2026Sale", "update.exe", "oneTime")]
    [InlineData("Daily", "tool.exe", "other")]
    public void ClassifiesLogonTasks(string task, string exe, string expected) =>
        Assert.Equal(expected, LogonTaskManager.Kind(task, exe, string.Format(TaskXml, "")));

    [Fact]
    public void OneTimeTriggerIsNotOverlay() => Assert.Equal("oneTime", LogonTaskManager.Kind("Daily", "tool.exe",
        string.Format(TaskXml, "<TimeTrigger><StartBoundary>2026-11-20T00:00:00</StartBoundary></TimeTrigger>")));

    [Fact]
    public void DisplayNameNeverLeaksPathOrQuotes()
    {
        Assert.Equal("HWiNFO64.exe", LogonTaskManager.ExeName("\"C:\\Program Files\\HWiNFO\\HWiNFO64.exe\" -a"));
        Assert.Equal("HWiNFO", LogonTaskManager.DisplayName("task", "HWiNFO64.exe", "\"HWiNFO\"", null));
        Assert.Equal("HWiNFO64", LogonTaskManager.DisplayName("task", "HWiNFO64.exe", "C:\\invalid", null));
    }

    [Theory]
    [InlineData(false, null, false, "not_managed")]
    [InlineData(false, null, true, "admin")]
    [InlineData(true, null, true, "admin")]
    [InlineData(true, null, false, null)]
    [InlineData(false, SeatAccountPurgePolicy.ManagedDescription, false, null)]
    public void RefusesUnmanagedAccountsAndAdministratorsBeforeAnyDeletion(
        bool managed, string? description, bool admin, string? expected) =>
        Assert.Equal(expected, SeatAccountPurgePolicy.InitialRefusal(managed, description, admin));

    [Theory]
    [InlineData("account_unknown", "not_found", "Compte introuvable")]
    [InlineData("not_managed", "forbidden", "Ce compte n'a pas été créé par Kinseat et ne peut pas être supprimé ici.")]
    [InlineData("admin", "forbidden", "Les comptes administrateurs ne peuvent pas être supprimés.")]
    [InlineData("identity_unknown", "forbidden", "Identité du compte non vérifiable : suppression refusée")]
    public void DeleteFaultDistinguishesMissingAndForbidden(string reason, string code, string message)
    {
        var fault = AdminPipeServer.AccountDeleteFault(reason);
        Assert.Equal(code, fault.Code);
        Assert.Equal(message, fault.Message);
    }

    [Fact]
    public async Task AccountDeleteRequiresOnlyValidAccountName()
    {
        var executed = false;
        async Task<object> Dispatch(string name) => await AdminDispatcher.DispatchAsync(
            Encoding.UTF8.GetBytes(JsonSerializer.Serialize(new { id = 1, method = "account.delete", @params = new { accountName = name } })),
            (_, _) => { executed = true; return Task.FromResult<object?>(new { ok = true }); });
        Assert.Contains("invalid_params", JsonSerializer.Serialize(await Dispatch("bad/name")));
        Assert.False(executed);
        Assert.Contains("ok", JsonSerializer.Serialize(await Dispatch("joueur")));
        Assert.True(executed);
        Assert.Equal(TimeSpan.FromSeconds(120), AdminPipeServer.RequestTimeoutFor("{\"method\":\"account.delete\"}"));
    }
}
