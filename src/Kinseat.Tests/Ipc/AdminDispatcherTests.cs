using System.Text;
using System.Text.Json;
using MultiSeat.Service.Ipc;
using Xunit;

namespace MultiSeat.Tests.Ipc;

public class AdminDispatcherTests
{
    private static JsonElement Send(string input, Func<string, object?>? handler = null) =>
        JsonSerializer.SerializeToElement(AdminDispatcher.Dispatch(Encoding.UTF8.GetBytes(input),
            handler ?? (_ => new { activeSeats = 0 })));

    [Theory]
    [InlineData("system.health")]
    [InlineData("seats.list")]
    public void DispatchesReadOnlyMethods(string method)
    {
        var called = false;
        var response = Send($"{{\"id\":7,\"method\":\"{method}\",\"params\":{{}}}}", m =>
        {
            Assert.Equal(method, m);
            called = true;
            return new { activeSeats = 0 };
        });
        Assert.True(called);
        Assert.Equal(7, response.GetProperty("id").GetInt32());
        Assert.Equal(0, response.GetProperty("result").GetProperty("activeSeats").GetInt32());
    }

    [Theory]
    [InlineData("{", "invalid_json")]
    [InlineData("{\"id\":1,\"method\":\"seats.list\"}", "invalid_request")]
    [InlineData("{\"id\":1,\"method\":\"seats.list\",\"params\":{\"secret\":1}}", "invalid_request")]
    [InlineData("{\"id\":1,\"method\":\"seats.delete\",\"params\":{}}", "unknown_method")]
    public void RejectsBadRequests(string input, string code)
    {
        var response = Send(input, _ => throw new Exception("Must not execute"));
        Assert.Equal(code, response.GetProperty("error").GetProperty("code").GetString());
        Assert.False(response.TryGetProperty("result", out _));
    }

    [Fact]
    public void InternalFailuresDoNotLeakExceptionMessages()
    {
        var response = Send("{\"id\":4,\"method\":\"system.health\",\"params\":{}}",
            _ => throw new Exception("sensitive detail"));
        Assert.Equal("internal_error", response.GetProperty("error").GetProperty("code").GetString());
        Assert.DoesNotContain("sensitive detail", response.ToString());
    }
}
