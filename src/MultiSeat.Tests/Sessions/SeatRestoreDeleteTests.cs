using MultiSeat.Service.Sessions;
using Xunit;

namespace MultiSeat.Tests.Sessions;

public sealed class SeatRestoreDeleteTests
{
    [Fact]
    public void DeletedDuringRestore_IsNoLongerWanted_ButOtherSeatsRemainWanted()
    {
        var removedId = Guid.NewGuid();
        var remainingId = Guid.NewGuid();
        IReadOnlySet<Guid> removed = new HashSet<Guid> { removedId };

        Assert.False(SeatManager.IsRestoreIdWanted(removed, removedId));
        Assert.True(SeatManager.IsRestoreIdWanted(removed, remainingId));
    }
}
