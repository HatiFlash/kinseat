using System.Text;
using System.Text.Json;
using MultiSeat.Service.Accounts;
using MultiSeat.Service.Gpu;
using MultiSeat.Service.Ipc;
using MultiSeat.Service.Sessions;
using MultiSeat.Shared.Models;
using Xunit;

namespace MultiSeat.Tests.Ipc;

public sealed class KinseatContractTests
{
    private const string Request = "00000000-0000-0000-0000-000000000002";
    private const string Seat = "00000000-0000-0000-0000-000000000001";

    private static async Task<JsonElement> Dispatch(string method, string parameters)
    {
        var line = Encoding.UTF8.GetBytes(JsonSerializer.Serialize(new { id = 1, method })[..^1] +
            $",\"params\":{parameters}}}");
        return JsonSerializer.SerializeToElement(await AdminDispatcher.DispatchAsync(line,
            (m, p) => Task.FromResult<object?>(new { method = m, parameters = p.Clone() })));
    }

    [Fact]
    public void AcceptOnExistingSeat_ProfileChangeTakesProfileBudget()
    {
        var seat = new SeatInfo { AccountName = "ks", GraphicsProfile = GraphicsProfile.Economy, VramBudgetMb = 2048 };
        var quality = JsonDocument.Parse("""{"graphicsProfile":"Quality"}""").RootElement;
        var changed = AdminPipeServer.ForExistingSeat(quality, seat);
        Assert.Equal((2560, 1440, 4096), (changed.Width, changed.Height, changed.BudgetMb));
        var same = JsonDocument.Parse("""{"graphicsProfile":"Economy","vramBudgetMb":null}""").RootElement;
        seat.VramBudgetMb = 2500;
        Assert.Equal(2500, AdminPipeServer.ForExistingSeat(same, seat).BudgetMb);
        var explicitBudget = JsonDocument.Parse("""{"graphicsProfile":"Quality","vramBudgetMb":3000}""").RootElement;
        Assert.Equal(3000, AdminPipeServer.ForExistingSeat(explicitBudget, seat).BudgetMb);
    }

    [Fact]
    public async Task JoinNullBudgetIsAbsentAndCustomValuesAreAccepted()
    {
        var plain = await Dispatch("joinRequest.accept", $"{{\"requestId\":\"{Request}\",\"seatId\":\"{Seat}\",\"vramBudgetMb\":null}}");
        Assert.Equal("joinRequest.accept", plain.GetProperty("result").GetProperty("method").GetString());
        var custom = await Dispatch("joinRequest.accept", $"{{\"requestId\":\"{Request}\",\"seatId\":\"{Seat}\",\"graphicsProfile\":\"Custom\",\"width\":2560,\"height\":1440,\"fps\":60,\"nvencPreset\":\"Quality\",\"vramBudgetMb\":null}}");
        var selection = GraphicsSelection.Parse(custom.GetProperty("result").GetProperty("parameters"));
        Assert.Equal(2560, selection.Width);
        // A null budget is inherited from the existing seat by ForExistingSeat, not parsed
        // as a new default (Custom has no standalone default budget).
        Assert.Equal(0, selection.BudgetMb);
        var bad = await Dispatch("joinRequest.accept", $"{{\"requestId\":\"{Request}\",\"seatId\":\"{Seat}\",\"vramBudgetMb\":\"oops\"}}");
        Assert.Equal("Profil graphique invalide", bad.GetProperty("error").GetProperty("message").GetString());
    }

    [Fact]
    public void DetectsOnlyInteractiveAnyUserTasks()
    {
        const string template = "<Task xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\"><Triggers><LogonTrigger>{0}</LogonTrigger></Triggers><Principals><Principal><UserId>alice</UserId><LogonType>{1}</LogonType></Principal></Principals><Actions><Exec><Command>C:\\Tools\\HWiNFO64Launcher.exe</Command></Exec></Actions></Task>";
        var any = LogonTaskManager.Detect(string.Format(template, "", "InteractiveToken"));
        Assert.Equal("alice", any.RunAs);
        Assert.False(any.Restricted);
        Assert.NotNull(any.Execute);
        Assert.True(LogonTaskManager.Detect(string.Format(template, "<UserId>alice</UserId>", "InteractiveToken")).Restricted);
        Assert.Null(LogonTaskManager.Detect(string.Format(template, "", "Password")).Execute);
        // Task Scheduler rewrites the restricted trigger as DOMAIN\\name while the principal stays a SID.
        var rewritten = LogonTaskManager.Detect(string.Format(template, "<UserId>PC\\autre</UserId>", "InteractiveToken"), restrictedByKinseat: true);
        Assert.True(rewritten.Restricted);
        Assert.NotNull(rewritten.Execute);
        Assert.True(LogonTaskManager.SameAccount("S-1-5-21-1-2-3-1006", "s-1-5-21-1-2-3-1006"));
        Assert.False(LogonTaskManager.SameAccount("S-1-5-21-1-2-3-1006", null));
        Assert.True(LogonTaskManager.Listed(restricted: false, hasKinseatBackup: false));
        Assert.True(LogonTaskManager.Listed(restricted: true, hasKinseatBackup: true));
        Assert.False(LogonTaskManager.Listed(restricted: true, hasKinseatBackup: false));
        Assert.True(LogonTaskManager.ValidPath(@"\", "HWiNFO"));
        Assert.False(LogonTaskManager.ValidPath(@"\Microsoft\Windows\", "HWiNFO"));
    }

    [Fact]
    public void ParseTaskNamesHandlesCsvQuotesCarriageReturnsAndFolders()
    {
        const string csv = @"""\HWiNFO"",""N/A"",""Prêt""" + "\r\n" + @"""\Vendor\Folder\Task A"",""N/A"",""Prêt""" + "\r\n" + @"""\vendor\folder\task a"",""N/A"",""En cours""" + "\r\n";
        Assert.Equal([@"\HWiNFO", @"\Vendor\Folder\Task A"], LogonTaskManager.ParseTaskNames(csv));
        Assert.Empty(LogonTaskManager.ParseTaskNames("\"Name without root\",\"Ready\"\r\n"));
        Assert.False(LogonTaskManager.ValidPath("", "HWiNFO"));
        Assert.False(LogonTaskManager.ValidPath(@"\", ""));
    }

    [Fact]
    public void LogonTaskRestrictionFaultsUseExpectedCodesAndMessages()
    {
        var missing = new AdminFault("not_found", "Tâche planifiée introuvable");
        Assert.Equal("not_found", missing.Code);
        Assert.Equal("Tâche planifiée introuvable", missing.Message);

        var invalid = new AdminFault("invalid_params", "Nom de tâche invalide");
        Assert.Equal("invalid_params", invalid.Code);
        Assert.Equal("Nom de tâche invalide", invalid.Message);
    }

    [Fact]
    public async Task TaskMutationsAreValidatedBeforeExecution()
    {
        var valid = await Dispatch("host.logonTasks.restrict", "{\"path\":\"\\\\\",\"name\":\"HWiNFO\"}");
        Assert.Equal("host.logonTasks.restrict", valid.GetProperty("result").GetProperty("method").GetString());
        var bad = await Dispatch("host.logonTasks.restore", "{\"path\":\"\\\\Microsoft\\\\\",\"name\":\"X\"}");
        Assert.Equal("invalid_params", bad.GetProperty("error").GetProperty("code").GetString());
    }

    [Fact]
    public async Task DeletePurgeFlagMustBeBoolean()
    {
        var valid = await Dispatch("seat.delete", $"{{\"seatId\":\"{Seat}\",\"purgeAccount\":true}}");
        Assert.Equal("seat.delete", valid.GetProperty("result").GetProperty("method").GetString());
        var bad = await Dispatch("seat.delete", $"{{\"seatId\":\"{Seat}\",\"purgeAccount\":null}}");
        Assert.Equal("invalid_params", bad.GetProperty("error").GetProperty("code").GetString());
    }

    [Fact]
    public void PurgeRequiresManagedAndEveryIdentityGuard()
    {
        Assert.True(SeatAccountPurgePolicy.CanDelete(true, null, false, false, false));
        Assert.True(SeatAccountPurgePolicy.CanDelete(false, SeatAccountPurgePolicy.ManagedDescription, false, false, false));
        Assert.False(SeatAccountPurgePolicy.CanDelete(false, null, false, false, false));
        Assert.False(SeatAccountPurgePolicy.CanDelete(true, null, true, false, false));
        Assert.False(SeatAccountPurgePolicy.CanDelete(true, null, false, true, false));
        Assert.False(SeatAccountPurgePolicy.CanDelete(true, null, false, false, true));
        Assert.False(SeatAccountPurgePolicy.CanDelete(false, "MultiSeat multi-seat account (managed) but not exact", false, false, false));
    }

    [Fact]
    public void VendorStopsAreBounded()
    {
        Assert.True(SeatLighteningService.MayStopVendor(0));
        Assert.True(SeatLighteningService.MayStopVendor(2));
        Assert.False(SeatLighteningService.MayStopVendor(3));
        Assert.False(SeatLighteningService.MayStopVendor(12));
    }

    [Fact]
    public void RdpStateChangesOnlyOnTransition()
    {
        var wrapper = new RdpWrapper(Microsoft.Extensions.Logging.Abstractions.NullLogger<RdpWrapper>.Instance);
        Assert.True(wrapper.StateChanged(true));
        Assert.False(wrapper.StateChanged(true));
        Assert.True(wrapper.StateChanged(false));
        Assert.False(wrapper.StateChanged(false));
    }

    [Fact]
    public void ExistingSeatPresetIsSkippedRegardlessOfAccountCase()
    {
        Assert.True(AutoStartProvisioner.HasSeatForAccount([new SeatInfo { AccountName = "Player" }], "player"));
        Assert.False(AutoStartProvisioner.HasSeatForAccount([], "player"));
    }
}
