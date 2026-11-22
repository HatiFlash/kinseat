using MultiSeat.Service.Sessions;
using Xunit;

namespace MultiSeat.Tests.Sessions;

public class SeatManagerRePairedKnownDeviceTests
{
    private static readonly DateTime T0 = new(2026, 11, 21, 20, 0, 0, DateTimeKind.Utc);

    [Fact]
    public void StateRewrittenWithTheDevice_IsAPairing()
        => Assert.Equal("KINSEAT-BUILD", SeatManager.RePairedKnownDeviceName(
            T0, T0.AddSeconds(2), new[] { "KINSEAT-BUILD", "labo-linux" }, "KINSEAT-BUILD"));

    [Fact]
    public void StateNotRewritten_IsNotAPairing()
        => Assert.Null(SeatManager.RePairedKnownDeviceName(
            T0, T0, new[] { "KINSEAT-BUILD" }, "KINSEAT-BUILD"));

    [Fact]
    public void StateRewrittenWithoutTheDevice_IsNotAPairing()
        => Assert.Null(SeatManager.RePairedKnownDeviceName(
            T0, T0.AddSeconds(2), new[] { "labo-linux" }, "KINSEAT-BUILD"));

    [Fact]
    public void ParenthesesAreMatchedAsApolloStoresThem()
        => Assert.Equal("PC [salon]", SeatManager.RePairedKnownDeviceName(
            T0, T0.AddSeconds(1), new[] { "PC [salon]" }, "PC (salon)"));

    [Fact]
    public void NoStateFileAfter_IsNotAPairing()
        => Assert.Null(SeatManager.RePairedKnownDeviceName(
            T0, null, new[] { "PC" }, "PC"));
}
