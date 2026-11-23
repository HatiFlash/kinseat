using MultiSeat.Service.Sessions;
using Xunit;

namespace MultiSeat.Tests.Sessions;

public class SeatManagerNewlyPairedNameTests
{
    [Fact]
    public void FirstDevice_IsReturned()
        => Assert.Equal("PC", SeatManager.NewlyPairedName(new string[0], new[] { "PC" }));

    [Fact]
    public void RenamedDuplicate_IsReturned_NotTheOlderDevice()
        => Assert.Equal("PC (2)", SeatManager.NewlyPairedName(new[] { "PC" }, new[] { "PC", "PC (2)" }));

    [Fact]
    public void SameNameTwice_ReturnsThatName()
        => Assert.Equal("PC", SeatManager.NewlyPairedName(new[] { "PC" }, new[] { "PC", "PC" }));

    [Fact]
    public void NothingAdded_ReturnsNull()
        => Assert.Null(SeatManager.NewlyPairedName(new[] { "A", "B" }, new[] { "B", "A" }));
}
