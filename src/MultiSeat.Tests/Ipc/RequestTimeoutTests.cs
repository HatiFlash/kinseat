using MultiSeat.Service.Ipc;
using Xunit;

namespace MultiSeat.Tests.Ipc;

public class RequestTimeoutTests
{
    private static string Line(string method) => $"{{\"id\":1,\"method\":\"{method}\",\"params\":{{}}}}";

    [Theory]
    [InlineData("seat.create")]
    [InlineData("seat.delete")]
    [InlineData("seat.resolution.set")]
    [InlineData("seat.preset.set")]
    [InlineData("seat.repair")]
    [InlineData("seat.action")]
    [InlineData("joinRequest.accept")]
    public void Long_methods_get_five_minutes(string method)
    {
        Assert.Equal(AdminPipeServer.LongRequestTimeout, AdminPipeServer.RequestTimeoutFor(Line(method)));
        Assert.Equal(TimeSpan.FromMinutes(5), AdminPipeServer.LongRequestTimeout);
    }

    [Fact]
    public void Pair_and_export_keep_their_own_budgets()
    {
        Assert.Equal(AdminPipeServer.PairRequestTimeout, AdminPipeServer.RequestTimeoutFor(Line("seat.pair")));
        Assert.Equal(TimeSpan.FromSeconds(60), AdminPipeServer.RequestTimeoutFor(Line("diagnostics.export")));
    }

    [Theory]
    [InlineData("seat.list")]
    [InlineData("seat.pairing")]
    [InlineData("seat.pair.cancel")]
    [InlineData("seat.created")]
    [InlineData("seat.actions")]
    [InlineData("app.authorization")]
    public void Other_methods_get_ten_seconds(string method)
    {
        Assert.Equal(TimeSpan.FromSeconds(10), AdminPipeServer.RequestTimeoutFor(Line(method)));
    }

    [Fact]
    public void Empty_line_gets_default()
    {
        Assert.Equal(AdminPipeServer.DefaultRequestTimeout, AdminPipeServer.RequestTimeoutFor(""));
    }
}
