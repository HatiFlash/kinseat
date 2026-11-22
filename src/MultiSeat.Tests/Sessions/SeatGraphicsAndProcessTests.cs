using MultiSeat.Service.Sessions;
using MultiSeat.Shared.Models;
using Xunit;

namespace MultiSeat.Tests.Sessions;

public class SeatGraphicsAndProcessTests
{
    [Fact]
    public void GraphicsAndLighteningSurviveAutoStartPreset()
    {
        var seat = new SeatInfo { AccountName = "placeA", GraphicsProfile = GraphicsProfile.Custom,
            Width = 1600, VramBudgetMb = 3500, Lightening = new SeatLightening { BlockVendorUtilities = false } };
        var preset = SeatPreset.ForAutoStart(seat);
        Assert.Equal(GraphicsProfile.Custom, preset.GraphicsProfile);
        Assert.Equal(3500, preset.VramBudgetMb);
        Assert.False(preset.Lightening.BlockVendorUtilities);
    }

    [Theory]
    [InlineData(GraphicsProfile.Economy, 1280, 720, NvencQualityPreset.Latency, 2048)]
    [InlineData(GraphicsProfile.Balanced, 1920, 1080, NvencQualityPreset.Balanced, 3072)]
    [InlineData(GraphicsProfile.Quality, 2560, 1440, NvencQualityPreset.Quality, 4096)]
    public void ProfileDefaults(GraphicsProfile profile, int width, int height, NvencQualityPreset nvenc, int mb)
    {
        var result = SeatGraphics.Defaults(profile);
        Assert.Equal((width, height, 60, nvenc, mb), result);
    }

    [Fact]
    public void BudgetRejectsOverflowAndUnknownCapacity()
    {
        Assert.True(SeatGraphics.Fits(8192, 2048, [2048], 4096));
        Assert.False(SeatGraphics.Fits(8192, 2048, [3072], 4096));
        Assert.False(SeatGraphics.Fits(0, 2048, [], 2048));
        Assert.False(SeatGraphics.Fits(8192, -1, [], 2048));
    }

    [Theory]
    [InlineData("explorer.exe")]
    [InlineData("sunshine.exe")]
    [InlineData("SecurityHealthSystray.exe")]
    [InlineData("MsMpEng.exe")]
    [InlineData("CrowdStrikeSensor.exe")]
    public void NeverStopsProtectedProcesses(string name) =>
        Assert.Null(SeatProcessClassifier.Classify(name, 7, 7, true, true, true));

    [Fact]
    public void OtherSessionAndUnknownSessionAreNeverTouched()
    {
        Assert.Null(SeatProcessClassifier.Classify("iCUE.exe", 4, 7, true, true, true));
        Assert.Null(SeatProcessClassifier.Classify("iCUE.exe", 0, 0, true, true, true));
        Assert.Null(SeatProcessClassifier.Classify("game.exe", 7, 7, true, true, true, gameProcess: true));
        Assert.Equal("vendor", SeatProcessClassifier.Classify("iCUE.exe", 7, 7, false, true, false));
        Assert.Equal("windhawk", SeatProcessClassifier.Classify("windhawk.exe", 7, 7, false, false, true));
    }

    [Fact]
    public void RestoredSeatKeepsSavedGraphicsProfile()
    {
        var seat = new SeatInfo { AccountName = "placeA" };
        var preset = SeatPreset.ForAutoStart(new SeatInfo { AccountName = "placeA",
            GraphicsProfile = GraphicsProfile.Economy, VramBudgetMb = 2048 });
        SeatManager.ApplySavedGraphics(seat, preset);
        Assert.Equal(GraphicsProfile.Economy, seat.GraphicsProfile);
        Assert.Equal(2048, seat.VramBudgetMb);
        var untouched = new SeatInfo { AccountName = "placeB" };
        SeatManager.ApplySavedGraphics(untouched, null);
        Assert.Equal(GraphicsProfile.Balanced, untouched.GraphicsProfile);
    }
}
