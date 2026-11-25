using System.Text;
using System.Text.Json;
using Microsoft.Win32;
using MultiSeat.Service.Ipc;
using MultiSeat.Service.Sessions;
using MultiSeat.Shared.Models;
using Xunit;

namespace MultiSeat.Tests.Sessions;

public sealed class SeatLighteningTests
{
    private const string Id = "00000000-0000-0000-0000-000000000001";

    [Fact]
    public void ApprovedBytesAndSecurityExclusions()
    {
        Assert.Equal(new byte[] { 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0 }, SeatLighteningService.Disabled);
        Assert.Equal(new byte[] { 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0 }, SeatLighteningService.Enabled);
        Assert.True(SeatProcessClassifier.IsSecurityStartup("SecurityHealth"));
        Assert.True(SeatProcessClassifier.IsSecurityStartup("Windows Defender"));
        Assert.True(SeatProcessClassifier.IsSecurityStartup("Bitdefender"));
        Assert.Null(SeatProcessClassifier.Classify("MsMpEng.exe", 4, 4, true, true, true));
    }

    [Fact]
    public void SelectProcessesNeverCrossesSessionOrKillsGames()
    {
        var processes = new[]
        {
            new SeatLighteningService.ProcessSnapshot(10, 3, "sunshine.exe", 0),
            new SeatLighteningService.ProcessSnapshot(11, 3, "game.exe", 10),
            new SeatLighteningService.ProcessSnapshot(12, 3, "helper.exe", 11),
            new SeatLighteningService.ProcessSnapshot(20, 3, "icue.exe", 0),
            new SeatLighteningService.ProcessSnapshot(21, 4, "icue.exe", 0),
            new SeatLighteningService.ProcessSnapshot(22, 3, "windhawk.exe", 0),
            new SeatLighteningService.ProcessSnapshot(23, 3, "explorer.exe", 0),
            new SeatLighteningService.ProcessSnapshot(24, 3, "thirdparty.exe", 0)
        };
        var result = SeatLighteningService.SelectProcesses(processes, 3, new SeatLightening(),
            new HashSet<string>(StringComparer.OrdinalIgnoreCase) { "thirdparty.exe", "helper.exe" }, 10, null)
            .Select(x => (x.Process.Pid, x.Category)).ToArray();
        Assert.Contains((20, "vendor"), result);
        Assert.Contains((22, "windhawk"), result);
        Assert.Contains((24, "startup"), result);
        Assert.Equal(3, result.Length);
    }

    [Fact]
    public void RestoreOnlyOwnBlockedValues()
    {
        if (!OperatingSystem.IsWindows()) return;
        var path = @"Software\KinseatTest-" + Guid.NewGuid().ToString("N");
        try
        {
            using var root = Registry.CurrentUser.CreateSubKey(path, true)!;
            var entries = new Dictionary<string, string> { ["Run|ThirdParty"] = "third.exe",
                ["Run32|SecurityHealth"] = "SecurityHealth.exe", ["StartupFolder|Extra.lnk"] = "extra.lnk" };
            const string approved = @"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
            using (var run = root.CreateSubKey(approved + @"\Run", true)!)
                run.SetValue("AlreadyDisabled", SeatLighteningService.Disabled, RegistryValueKind.Binary);
            SeatLighteningService.ApplyApproved(root, entries, true);
            using (var run = root.OpenSubKey(approved + @"\Run", true)!)
            {
                Assert.Equal(SeatLighteningService.Disabled, run.GetValue("ThirdParty") as byte[]);
                run.SetValue("ThirdParty", new byte[] { 9, 0, 0, 0 }, RegistryValueKind.Binary);
            }
            SeatLighteningService.ApplyApproved(root, entries, false);
            using var final = root.OpenSubKey(approved + @"\Run")!;
            Assert.Equal(new byte[] { 9, 0, 0, 0 }, final.GetValue("ThirdParty") as byte[]);
            Assert.Equal(SeatLighteningService.Disabled, final.GetValue("AlreadyDisabled") as byte[]);
            using var run32 = root.OpenSubKey(approved + @"\Run32")!;
            Assert.Null(run32.GetValue("SecurityHealth"));
        }
        finally { Registry.CurrentUser.DeleteSubKeyTree(path, false); }
    }

    [Fact]
    public void ApplyPowerPolicyHidesShutdownForSeatAccount()
    {
        if (!OperatingSystem.IsWindows()) return;
        var path = @"Software\KinseatTest-" + Guid.NewGuid().ToString("N");
        try
        {
            using var root = Registry.CurrentUser.CreateSubKey(path, true)!;
            SeatLighteningService.ApplyPowerPolicy(root);
            SeatLighteningService.ApplyPowerPolicy(root);
            using var policy = root.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer")!;
            Assert.Equal(1, policy.GetValue("NoClose"));
            Assert.Equal(RegistryValueKind.DWord, policy.GetValueKind("NoClose"));
        }
        finally { Registry.CurrentUser.DeleteSubKeyTree(path, false); }
    }

    [Fact]
    public void ApplySeatSafetyPolicySetsValuesIdempotentlyInsideRootOnly()
    {
        if (!OperatingSystem.IsWindows()) return;
        var path = @"Software\KinseatTest-" + Guid.NewGuid().ToString("N");
        const string pol = @"Software\Microsoft\Windows\CurrentVersion\Policies";
        try
        {
            using var root = Registry.CurrentUser.CreateSubKey(path, true)!;
            using (var pre = root.CreateSubKey(pol + @"\Explorer\DisallowRun", true)!)
                pre.SetValue("1", "custom.exe", RegistryValueKind.String);
            SeatLighteningService.ApplySeatSafetyPolicy(root);
            SeatLighteningService.ApplySeatSafetyPolicy(root);
            foreach (var (sub, name) in new[] { ("Explorer", "HideSCANetwork"), ("Explorer", "NoControlPanel"),
                ("Explorer", "DisallowRun"), ("System", "DisableTaskMgr"), ("System", "DisableRegistryTools") })
            {
                using var key = root.OpenSubKey(pol + @"\" + sub)!;
                Assert.Equal(1, key.GetValue(name));
                Assert.Equal(RegistryValueKind.DWord, key.GetValueKind(name));
            }
            using var list = root.OpenSubKey(pol + @"\Explorer\DisallowRun")!;
            var values = list.GetValueNames().Select(n => (string)list.GetValue(n)!).ToArray();
            Assert.Equal("custom.exe", list.GetValue("1"));
            Assert.Equal(SeatLighteningService.DisallowedPrograms.Length + 1, values.Length);
            foreach (var program in SeatLighteningService.DisallowedPrograms) Assert.Contains(program, values);
            Assert.DoesNotContain("cmd.exe", values);
            using var explorer = root.OpenSubKey(pol + @"\Explorer")!;
            Assert.Null(explorer.GetValue("NoLogoff"));
            Assert.Null(explorer.GetValue("StartMenuLogOff"));
            // Nothing outside the root hive: no cmd policy written under it either.
            Assert.Null(root.OpenSubKey(@"Software\Policies\Microsoft\Windows\System"));
        }
        finally { Registry.CurrentUser.DeleteSubKeyTree(path, false); }
    }

    [Theory]
    [InlineData("seat.lightening.set", "{}")]
    [InlineData("seat.lightening.set", "{\"emptyDesktop\":true,\"blockStartupApps\":true,\"blockVendorUtilities\":true,\"closeWindhawkTray\":0}")]
    [InlineData("seat.lightening.report", "{\"extra\":true}")]
    public async Task PipeRejectsBadParameters(string method, string fields)
    {
        var values = fields == "{}" ? $"{{\"seatId\":\"{Id}\"}}" :
            $"{{\"seatId\":\"{Id}\",{fields[1..]}";
        var line = Encoding.UTF8.GetBytes($"{{\"id\":1,\"method\":\"{method}\",\"params\":{values}}}");
        var reply = JsonSerializer.SerializeToElement(await AdminDispatcher.DispatchAsync(line,
            (_, _) => throw new Exception("Should not dispatch")));
        Assert.Equal("invalid_params", reply.GetProperty("error").GetProperty("code").GetString());
    }

    [Fact]
    public async Task PipeAcceptsValidatedSettings()
    {
        var line = Encoding.UTF8.GetBytes($"{{\"id\":1,\"method\":\"seat.lightening.set\",\"params\":{{\"seatId\":\"{Id}\",\"emptyDesktop\":true,\"blockStartupApps\":false,\"blockVendorUtilities\":true,\"closeWindhawkTray\":false}}}}");
        var reply = JsonSerializer.SerializeToElement(await AdminDispatcher.DispatchAsync(line,
            (method, _) => Task.FromResult<object?>(new { method })));
        Assert.Equal("seat.lightening.set", reply.GetProperty("result").GetProperty("method").GetString());
    }

    [Theory]
    [InlineData("\"C:\\Program Files\\Windhawk\\windhawk.exe\" -tray-only", "windhawk.exe")]
    [InlineData("C:\\Program Files\\Corsair\\iCUE.exe --autorun", "iCUE.exe")]
    [InlineData("\"C:\\Users\\x\\AppData\\Local\\Microsoft\\OneDrive\\OneDrive.exe\" /background", "OneDrive.exe")]
    [InlineData("rundll32.exe shell32.dll,Control_RunDLL", "rundll32.exe")]
    [InlineData("C:\\Tools\\script.cmd", null)]
    public void ExecutableOfHandlesQuotedPathsWithSpaces(string command, string? expected) =>
        Assert.Equal(expected, SeatLighteningService.ExecutableOf(command));

    [Fact]
    public void SeatRunEntriesSkipKinseatSteamAndSecurity()
    {
        if (!OperatingSystem.IsWindows()) return;
        var path = @"Software\KinseatTest-" + Guid.NewGuid().ToString("N");
        try
        {
            using var root = Registry.CurrentUser.CreateSubKey(path, true)!;
            using (var run = root.CreateSubKey(@"Software\Microsoft\Windows\CurrentVersion\Run", true)!)
            {
                run.SetValue("OneDrive", "\"C:\\OneDrive\\OneDrive.exe\" /background");
                run.SetValue("Steam", "C:\\Program Files\\Kinseat\\Kinseat.SteamShim.exe");
                run.SetValue("Agent", "C:\\Program Files\\Kinseat\\kinseat-agent.exe");
                run.SetValue("SecurityHealth", "SecurityHealthSystray.exe");
            }
            var entries = SeatLighteningService.SeatRunEntries(root);
            Assert.Equal(new[] { "Run|OneDrive" }, entries.Keys.ToArray());
        }
        finally { Registry.CurrentUser.DeleteSubKeyTree(path, false); }
    }
}
