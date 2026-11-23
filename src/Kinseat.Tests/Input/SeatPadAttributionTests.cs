using MultiSeat.Service.Input;
using Xunit;

namespace MultiSeat.Tests.Input;

public class SeatPadAttributionTests
{
    private static readonly DateTimeOffset T0 = new(2026, 11, 15, 0, 0, 0, TimeSpan.Zero);

    private static HidHideDevice Pad(string serial) => new()
    {
        DeviceInstancePath = $@"HID\VID_045E&PID_028E&IG_00\3&X&0&{serial}",
        BaseContainerDeviceInstancePath = $@"USB\VID_045E&PID_028E\{serial}",
    };

    [Fact]
    public void Pad_that_appeared_after_the_client_connected_goes_to_that_seat()
    {
        var seat = Guid.NewGuid();
        var pad = Pad("01");
        var plan = HidHideConfigurator.PlanSeatPad([(seat, T0)], [(pad, T0.AddSeconds(3))], 5000);
        Assert.Equal(seat, plan.Seat);
        Assert.Same(pad, plan.Pad);
    }

    [Fact]
    public void A_pad_that_was_already_there_is_never_taken()
    {
        var plan = HidHideConfigurator.PlanSeatPad([(Guid.NewGuid(), T0)], [(Pad("01"), T0.AddMinutes(-5))], 5000);
        Assert.Null(plan.Seat);
        Assert.NotNull(plan.Reason);
    }

    [Fact]
    public void Same_tick_arrival_is_accepted()
    {
        var plan = HidHideConfigurator.PlanSeatPad([(Guid.NewGuid(), T0)], [(Pad("01"), T0.AddSeconds(-4))], 5000);
        Assert.NotNull(plan.Pad);
    }

    [Fact]
    public void Two_waiting_seats_or_two_new_pads_confine_nothing()
    {
        var twoSeats = HidHideConfigurator.PlanSeatPad(
            [(Guid.NewGuid(), T0), (Guid.NewGuid(), T0)], [(Pad("01"), T0.AddSeconds(1))], 5000);
        Assert.Null(twoSeats.Pad);
        var twoPads = HidHideConfigurator.PlanSeatPad(
            [(Guid.NewGuid(), T0)], [(Pad("01"), T0.AddSeconds(1)), (Pad("02"), T0.AddSeconds(2))], 5000);
        Assert.Null(twoPads.Pad);
        Assert.NotNull(twoPads.Reason);
    }

    [Fact]
    public void Same_path_recreated_after_restart_is_reconfined_to_its_previous_seat()
    {
        var seat = Guid.NewGuid();
        var pad = Pad("01");
        var remembered = new Dictionary<Guid, string> { [seat] = pad.DeviceInstancePath.ToLowerInvariant() };
        var plan = HidHideConfigurator.PlanSeatPad(
            [(seat, T0)], [(pad, T0.AddMinutes(-10))], 5000, remembered);
        Assert.Equal(seat, plan.Seat);
        Assert.Same(pad, plan.Pad);
    }

    [Fact]
    public void Previously_confined_pad_of_another_seat_is_never_taken_even_when_fresh()
    {
        var owner = Guid.NewGuid();
        var waiting = Guid.NewGuid();
        var pad = Pad("01");
        var remembered = new Dictionary<Guid, string> { [owner] = pad.DeviceInstancePath };
        var plan = HidHideConfigurator.PlanSeatPad(
            [(waiting, T0)], [(pad, T0.AddSeconds(1))], 5000, remembered);
        Assert.Null(plan.Pad);
    }

    [Fact]
    public void Two_waiting_seats_still_confine_nothing_with_a_remembered_pad()
    {
        var owner = Guid.NewGuid();
        var other = Guid.NewGuid();
        var pad = Pad("01");
        var plan = HidHideConfigurator.PlanSeatPad(
            [(owner, T0), (other, T0)], [(pad, T0.AddMinutes(-10))], 5000,
            new Dictionary<Guid, string> { [owner] = pad.DeviceInstancePath });
        Assert.Null(plan.Pad);
    }

    [Fact]
    public void Remembered_pad_does_not_override_ambiguity_with_another_new_pad()
    {
        var owner = Guid.NewGuid();
        var pad = Pad("01");
        var plan = HidHideConfigurator.PlanSeatPad(
            [(owner, T0)], [(pad, T0.AddMinutes(-10)), (Pad("02"), T0.AddSeconds(1))], 5000,
            new Dictionary<Guid, string> { [owner] = pad.DeviceInstancePath });
        Assert.Null(plan.Pad);
    }

    [Fact]
    public void Nobody_waiting_is_silent()
    {
        var plan = HidHideConfigurator.PlanSeatPad([], [(Pad("01"), T0)], 5000);
        Assert.Equal(default, plan);
    }
}
