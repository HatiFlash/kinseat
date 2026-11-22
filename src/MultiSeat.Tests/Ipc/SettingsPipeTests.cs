using System.Text;
using System.Text.Json;
using Microsoft.AspNetCore.Http;
using MultiSeat.Service.Ipc;
using Xunit;

namespace MultiSeat.Tests.Ipc;

public class SettingsPipeTests
{
    private const string Seat = "\"seatId\":\"00000000-0000-0000-0000-000000000001\"";

    private static async Task<JsonElement> Dispatch(string method, string parameters,
        Func<string, JsonElement, Task<object?>>? execute = null)
    {
        var bytes = Encoding.UTF8.GetBytes($"{{\"id\":9,\"method\":\"{method}\",\"params\":{parameters}}}");
        return JsonSerializer.SerializeToElement(await AdminDispatcher.DispatchAsync(bytes,
            execute ?? ((_, _) => throw new Exception("Invalid request reached service"))));
    }

    [Theory]
    [InlineData("host.refreshRate.get", "{\"x\":1}")]
    [InlineData("host.refreshRate.set", "{\"intervalMs\":4}")]
    [InlineData("host.refreshRate.set", "{\"intervalMs\":\"8\"}")]
    [InlineData("seat.autostart.set", "{" + Seat + ",\"enabled\":\"yes\"}")]
    [InlineData("seat.preset.set", "{" + Seat + ",\"preset\":\"Ultra\"}")]
    [InlineData("seat.resolution.set", "{" + Seat + ",\"width\":100,\"height\":1080}")]
    [InlineData("seat.resolution.set", "{" + Seat + ",\"width\":1920}")]
    [InlineData("seat.resolution.set", "{" + Seat + ",\"width\":\"1920\",\"height\":1080}")]
    [InlineData("seat.repair", "{" + Seat + ",\"target\":\"disk\"}")]
    [InlineData("seat.repair", "{\"seatId\":\"nope\",\"target\":\"audio\"}")]
    public async Task InvalidParametersNeverExecute(string method, string parameters)
    {
        var result = await Dispatch(method, parameters);
        Assert.Equal("invalid_params", result.GetProperty("error").GetProperty("code").GetString());
    }

    [Theory]
    [InlineData("host.refreshRate.get", "{}")]
    [InlineData("host.refreshRate.set", "{\"intervalMs\":16}")]
    [InlineData("seat.autostart.set", "{" + Seat + ",\"enabled\":true}")]
    [InlineData("seat.preset.set", "{" + Seat + ",\"preset\":\"Quality\"}")]
    [InlineData("seat.resolution.set", "{" + Seat + ",\"width\":1920,\"height\":1080}")]
    [InlineData("seat.repair", "{" + Seat + ",\"target\":\"controller\"}")]
    public async Task DispatchesValidMethods(string method, string parameters)
    {
        var result = await Dispatch(method, parameters, (actual, _) =>
        {
            Assert.Equal(method, actual);
            return Task.FromResult<object?>(new { ok = true });
        });
        Assert.True(result.GetProperty("result").GetProperty("ok").GetBoolean());
    }

    [Fact]
    public void UnwrapReturnsValueOfSuccess() =>
        Assert.Equal(16, JsonSerializer.SerializeToElement(AdminResults.Unwrap(Results.Ok(new { intervalMs = 16 })))
            .GetProperty("intervalMs").GetInt32());

    [Fact]
    public void UnwrapTurnsErrorsIntoAdminFaults()
    {
        var bad = Assert.Throws<AdminFault>(() => AdminResults.Unwrap(Results.BadRequest(new { error = "non" })));
        Assert.Equal("invalid_params", bad.Code);
        Assert.Equal("non", bad.Message);
        var failed = Assert.Throws<AdminFault>(() =>
            AdminResults.Unwrap(Results.Json(new { error = "registre" }, statusCode: 500)));
        Assert.Equal("unavailable", failed.Code);
    }
}
