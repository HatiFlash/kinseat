using System.Text;
using System.Text.Json;
using MultiSeat.Service.Ipc;
using Xunit;

namespace MultiSeat.Tests.Ipc;

public class PairingPipeTests
{
    private static async Task<JsonElement> Dispatch(string method, string parameters,
        Func<string, JsonElement, Task<object?>>? execute = null)
    {
        var line = Encoding.UTF8.GetBytes($"{{\"id\":1,\"method\":\"{method}\",\"params\":{parameters}}}");
        var result = await AdminDispatcher.DispatchAsync(line, execute ?? ((_, _) =>
            throw new Exception("Invalid parameters reached service")));
        return JsonSerializer.SerializeToElement(result);
    }

    private const string Seat = "00000000-0000-0000-0000-000000000001";

    [Theory]
    [InlineData("seat.pair", "{\"seatId\":\"00000000-0000-0000-0000-000000000001\",\"pin\":\"123\",\"name\":\"Test\"}")]
    [InlineData("seat.pair", "{\"seatId\":\"00000000-0000-0000-0000-000000000001\",\"pin\":\"١٢٣٤\",\"name\":\"Test\"}")]
    [InlineData("seat.pair", "{\"seatId\":\"00000000-0000-0000-0000-000000000001\",\"pin\":\"1234\",\"name\":\"\"}")]
    [InlineData("seat.clients.remove", "{\"seatId\":\"00000000-0000-0000-0000-000000000001\",\"name\":\"\"}")]
    [InlineData("seat.clients.list", "{\"seatId\":\"bad\"}")]
    public async Task InvalidInputNeverExecutes(string method, string parameters)
    {
        var response = await Dispatch(method, parameters);
        Assert.Equal("invalid_params", response.GetProperty("error").GetProperty("code").GetString());
    }

    [Theory]
    [InlineData("seat.pair", "not_found")]
    [InlineData("seat.pair", "conflict")]
    public async Task PropagatesSeatFaults(string method, string code)
    {
        var response = await Dispatch(method, $"{{\"seatId\":\"{Seat}\",\"pin\":\"0123\",\"name\":\"Test\"}}",
            (_, _) => throw new AdminFault(code, "Seat unavailable"));
        Assert.Equal(code, response.GetProperty("error").GetProperty("code").GetString());
    }

    [Fact]
    public async Task AllMethodsReachDispatcher()
    {
        foreach (var (method, parameters) in new[] {
            ("seat.pair", $"{{\"seatId\":\"{Seat}\",\"pin\":\"0123\",\"name\":\"Test\"}}"),
            ("seat.clients.list", $"{{\"seatId\":\"{Seat}\"}}"),
            ("seat.clients.remove", $"{{\"seatId\":\"{Seat}\",\"name\":\"Test\"}}") })
        {
            var response = await Dispatch(method, parameters, (actual, _) => {
                Assert.Equal(method, actual);
                return Task.FromResult<object?>(new { ok = true });
            });
            Assert.True(response.GetProperty("result").GetProperty("ok").GetBoolean());
        }
    }

    [Fact]
    public void PipeWaitsLongerThanTheDeviceCheck()
    {
        // A wrong code is only known once the device gave up; the pipe must still be open to say so.
        Assert.True(AdminPipeServer.PairRequestTimeout >= MultiSeat.Service.Sessions.SeatManager.PairDeviceWait + TimeSpan.FromSeconds(15));
    }
}
