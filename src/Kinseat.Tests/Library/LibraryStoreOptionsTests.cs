using System.Security.Principal;
using System.Text.Json;
using Microsoft.Extensions.Logging.Abstractions;
using MultiSeat.Service.Library;
using Xunit;

namespace MultiSeat.Tests.Library;

public sealed class LibraryStoreOptionsTests : IDisposable
{
    private readonly string _root = Path.Combine(Path.GetTempPath(), "library-options-" + Guid.NewGuid().ToString("N"));

    public LibraryStoreOptionsTests() => Directory.CreateDirectory(_root);

    public void Dispose()
    {
        try { Directory.Delete(_root, recursive: true); } catch (IOException) { } catch (UnauthorizedAccessException) { }
    }

    [Fact]
    public async Task SetOptionsResyncsManifestOfAssignedAccounts()
    {
        if (!OperatingSystem.IsWindows()) return;
        var gameDir = Path.Combine(_root, "Longshot");
        Directory.CreateDirectory(Path.Combine(gameDir, "user", "save", "id"));
        var exe = Path.Combine(gameDir, "longshot.exe");
        File.WriteAllBytes(exe, [0x4D, 0x5A]);
        Directory.CreateDirectory(Path.Combine(_root, "profile"));
        Directory.CreateDirectory(Path.Combine(_root, "public"));

        var store = new LibraryStore(Path.Combine(_root, "library.json"));
        var id = store.Add("Longshot", exe).Games.Single().Id;
        store.Assign(id, "seat", true);
        store.SetOptions(id, null, true);
        var writer = new GameLaunchManifestWriter(_ => true,
            _ => (WindowsIdentity.GetCurrent().User!.Value, Path.Combine(_root, "profile")),
            Path.Combine(_root, "games"), Path.Combine(_root, "public"), isPortable: _ => true);
        var manifest = Assert.Single(writer.Sync("seat", store.Get().Games)).Value;
        using (var initial = JsonDocument.Parse(File.ReadAllText(manifest)))
            Assert.True(initial.RootElement.GetProperty("copyFromHostOnce").GetBoolean());

        var scanners = new GameScannerRunner([], NullLogger<GameScannerRunner>.Instance);
        var handler = new LibraryIpcHandler(store, scanners, manifests: writer);
        var assignmentsChanged = false;
        handler.AssignmentsChanged += _ => assignmentsChanged = true;
        using var parameters = JsonDocument.Parse($$"""{"gameId":"{{id}}","startFromHostSave":false}""");
        await handler.HandleAsync("library.setOptions", parameters.RootElement, CancellationToken.None);

        using var updated = JsonDocument.Parse(File.ReadAllText(manifest));
        Assert.False(updated.RootElement.GetProperty("copyFromHostOnce").GetBoolean());
        Assert.Contains(updated.RootElement.GetProperty("redirects").EnumerateArray(),
            r => r.GetProperty("mode").GetString() == "redirect"
                && r.GetProperty("from").GetString() == Path.Combine(gameDir, "user", "save"));
        Assert.False(assignmentsChanged);
    }
}
