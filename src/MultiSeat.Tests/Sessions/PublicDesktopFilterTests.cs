using System.Security.AccessControl;
using System.Security.Principal;
using Microsoft.Extensions.Logging.Abstractions;
using MultiSeat.Service.Configuration;
using MultiSeat.Service.Library;
using MultiSeat.Service.Sessions;
using MultiSeat.Service.SteamShim;
using Xunit;

namespace MultiSeat.Tests.Sessions;

public sealed class PublicDesktopFilterTests
{
    [Fact]
    public void ManagedDeny_IsIdempotent_AndRemovedOnlyWhenInventoried()
    {
        if (!OperatingSystem.IsWindows()) return;
        var root = Path.Combine(Path.GetTempPath(), "kinseat-public-desktop-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(root);
        var sid = WindowsIdentity.GetCurrent().User!.Value;
        var inventory = Path.Combine(root, "inventory.json");
        var desktop = Path.Combine(root, "desktop");
        Directory.CreateDirectory(desktop);
        var filter = new PublicDesktopFilter(desktop, inventory);
        try
        {
            filter.Apply(sid, "player", true, false);
            Assert.False(File.Exists(inventory));
            filter.Apply(sid, "player", true, true);
            filter.Apply(sid, "player", true, true);
            Assert.Single(System.Text.Json.JsonDocument.Parse(File.ReadAllText(inventory)).RootElement.EnumerateArray());
            filter.Apply(sid, "player", false, false);
            Assert.Empty(System.Text.Json.JsonDocument.Parse(File.ReadAllText(inventory)).RootElement.EnumerateArray());
            var info = new DirectoryInfo(desktop);
            var acl = info.GetAccessControl();
            var foreign = new FileSystemAccessRule(new SecurityIdentifier(sid), FileSystemRights.ReadData,
                InheritanceFlags.None, PropagationFlags.None, AccessControlType.Deny);
            acl.AddAccessRule(foreign);
            info.SetAccessControl(acl);
            filter.Apply(sid, "player", false, false);
            Assert.Contains(info.GetAccessControl().GetAccessRules(true, false, typeof(SecurityIdentifier))
                .Cast<FileSystemAccessRule>(), rule => rule.AccessControlType == AccessControlType.Deny &&
                    rule.IdentityReference.Value == sid);
        }
        finally { try { Directory.Delete(root, true); } catch (UnauthorizedAccessException) { } }
    }

    [Fact]
    public void BrowserPreference_UsesFallbackWhenBraveMissing()
    {
        var root = Path.Combine(Path.GetTempPath(), "kinseat-browser-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(root);
        var brave = Path.Combine(root, "brave.exe");
        var fallback = Path.Combine(root, "default.exe");
        try
        {
            File.WriteAllText(fallback, "");
            Assert.Equal(fallback, DesktopBrowserResolver.Choose(() => brave, () => fallback));
            File.WriteAllText(brave, "");
            Assert.Equal(brave, DesktopBrowserResolver.Choose(() => brave, () => fallback));
        }
        finally { Directory.Delete(root, true); }
    }
}
