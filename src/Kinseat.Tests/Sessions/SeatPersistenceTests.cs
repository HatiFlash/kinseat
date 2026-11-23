using Microsoft.Extensions.Logging.Abstractions;
using MultiSeat.Service.Sessions;
using MultiSeat.Service.Storage;
using MultiSeat.Shared.Models;
using Xunit;

namespace MultiSeat.Tests.Sessions;

public sealed class SeatPersistenceTests
{
    private static SeatDefinitionStore Store(string path) =>
        new(NullLogger<SeatDefinitionStore>.Instance, path);

    [Fact]
    public void RoundTrip_AndDeletion_PreserveDefinitionAndRemoveIt()
    {
        var folder = Path.Combine(Path.GetTempPath(), "seat-persist-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(folder);
        var path = Path.Combine(folder, "seats.json");
        try
        {
            var original = new SeatInfo
            {
                AccountName = "ks-test1", Width = 2560, Height = 1440,
                Fps = 120, PortBase = 48130, AutoStart = true,
                NvencPreset = NvencQualityPreset.Quality, ScaleFactorOverride = 150,
                LaunchApp = "game.exe"
            };
            Store(path).Upsert(original);
            var restored = Assert.Single(Store(path).GetAll());
            Assert.Equal(original.Id, restored.Id);
            Assert.Equal(original.AccountName, restored.AccountName);
            Assert.Equal(original.Width, restored.Width);
            Assert.Equal(original.Height, restored.Height);
            Assert.Equal(original.Fps, restored.Fps);
            Assert.Equal(original.PortBase, restored.PortBase);
            Assert.Equal(original.AutoStart, restored.AutoStart);
            Assert.Equal(original.NvencPreset, restored.NvencPreset);
            Assert.Equal(original.ScaleFactorOverride, restored.ScaleFactorOverride);
            Assert.Equal(original.LaunchApp, restored.LaunchApp);
            Store(path).Remove(original.Id);
            Assert.Empty(Store(path).GetAll());
        }
        finally { Directory.Delete(folder, recursive: true); }
    }

    [Fact]
    public void CorruptFile_IsQuarantined_AndDoesNotCrashStartup()
    {
        var folder = Path.Combine(Path.GetTempPath(), "seat-persist-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(folder);
        var path = Path.Combine(folder, "seats.json");
        try
        {
            File.WriteAllText(path, "{broken");
            Assert.Empty(Store(path).GetAll());
            Assert.False(File.Exists(path));
            Assert.Equal("{broken", File.ReadAllText(path + ".bad"));
        }
        finally { Directory.Delete(folder, recursive: true); }
    }

    [Fact]
    public async Task ActiveSession_ReusesId_WithoutLaunchingAnotherSession()
    {
        var id = Guid.NewGuid();
        var definition = new SeatDefinition(id, "ks-test1", 1920, 1080, 60, 48100,
            null, NvencQualityPreset.Balanced, null, false);
        var adoptCount = 0;
        var launchCount = 0;
        var restored = await SeatManager.RestoreDecisionAsync(definition,
            _ => 4,
            (seat, _) => { adoptCount++; seat.Status = SeatStatus.Ready; return Task.CompletedTask; },
            (_, _) => { launchCount++; return Task.CompletedTask; },
            CancellationToken.None);
        Assert.Equal(id, restored.Id);
        Assert.Equal(4, restored.SessionId);
        Assert.Equal(1, adoptCount);
        Assert.Equal(0, launchCount);
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task MissingSession_WaitsOrAutoStarts_WithoutAdoption(bool autoStart)
    {
        var id = Guid.NewGuid();
        var definition = new SeatDefinition(id, "ks-test1", 1920, 1080, 60, 48100,
            null, NvencQualityPreset.Balanced, null, autoStart);
        var launched = false;
        var restored = await SeatManager.RestoreDecisionAsync(definition, _ => -1,
            (_, _) => throw new Xunit.Sdk.XunitException("Never adopt a missing session"),
            (seat, _) => { launched = true; Assert.Equal(id, seat.Id); return Task.CompletedTask; },
            CancellationToken.None);
        Assert.Equal(autoStart, launched);
        if (!autoStart) Assert.Equal(SeatStatus.Idle, restored.Status);
    }
}
