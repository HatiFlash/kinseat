using System.Text;
using System.Text.Json;
using MultiSeat.Service;
using MultiSeat.Service.Ipc;
using Xunit;

namespace MultiSeat.Tests.Ipc;

public class SeatsPipeTests
{
    private const string Id = "00000000-0000-0000-0000-000000000001";

    private static async Task<JsonElement> Dispatch(string method, string parameters,
        Func<string, JsonElement, Task<object?>>? execute = null)
    {
        var bytes = Encoding.UTF8.GetBytes($"{{\"id\":7,\"method\":\"{method}\",\"params\":{parameters}}}");
        return JsonSerializer.SerializeToElement(await AdminDispatcher.DispatchAsync(bytes,
            execute ?? ((_, _) => throw new Exception("Invalid request reached service"))));
    }

    [Theory]
    [InlineData("accounts.list", "{\"unexpected\":1}")]
    [InlineData("account.create", "{\"name\":\"bad name\",\"password\":\"secret\"}")]
    [InlineData("account.create", "{\"name\":\"player\",\"password\":\"  \"}")]
    [InlineData("seat.create", "{\"accountName\":\"../bad\"}")]
    [InlineData("seat.create", "{\"accountName\":\"player\",\"preset\":\"Unknown\"}")]
    [InlineData("seat.create", "{\"accountName\":\"player\",\"displayName\":\"\"}")]
    [InlineData("seat.delete", "{\"seatId\":\"invalid\"}")]
    public async Task InvalidParametersNeverExecute(string method, string parameters)
    {
        var result = await Dispatch(method, parameters);
        Assert.Equal("invalid_params", result.GetProperty("error").GetProperty("code").GetString());
    }

    [Theory]
    [InlineData("accounts.list", "{}")]
    [InlineData("account.create", "{\"name\":\"player\",\"password\":\"secret\"}")]
    [InlineData("seat.create", "{\"accountName\":\"player\",\"displayName\":\"Camille\",\"preset\":\"Balanced\"}")]
    [InlineData("seat.delete", "{\"seatId\":\"00000000-0000-0000-0000-000000000001\"}")]
    public async Task DispatchesValidMethods(string method, string parameters)
    {
        var result = await Dispatch(method, parameters, (actual, _) =>
        {
            Assert.Equal(method, actual);
            return Task.FromResult<object?>(new { ok = true });
        });
        Assert.Equal(7UL, result.GetProperty("id").GetUInt64());
        Assert.True(result.GetProperty("result").GetProperty("ok").GetBoolean());
    }

    [Theory]
    [InlineData("not_found")]
    [InlineData("conflict")]
    public async Task PropagatesFaults(string code)
    {
        var result = await Dispatch("seat.delete", $"{{\"seatId\":\"{Id}\"}}",
            (_, _) => throw new AdminFault(code, "Refus"));
        Assert.Equal(code, result.GetProperty("error").GetProperty("code").GetString());
    }

    [Fact]
    public async Task CapacityIsConflict()
    {
        var result = await Dispatch("seat.create", "{\"accountName\":\"player\"}",
            (_, _) => throw new CapacityExhaustedException("Full"));
        Assert.Equal("conflict", result.GetProperty("error").GetProperty("code").GetString());
    }
}
