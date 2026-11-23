using System.Text;
using System.Text.Json;
using Microsoft.Extensions.Logging.Abstractions;
using MultiSeat.Service.Gpu;
using MultiSeat.Service.Ipc;
using MultiSeat.Shared.Models;
using Xunit;

namespace MultiSeat.Tests.Sessions;

public sealed class GpuBudgetTests
{
    private static readonly string SeatId = Guid.NewGuid().ToString();

    [Fact]
    public void CapacityRejectsAndReportsAvailableMb()
    {
        var path = Path.Combine(Path.GetTempPath(), Guid.NewGuid().ToString("N"), "gpu.json");
        var gpu = new GpuBudgetService(() => ("Test GPU", 8192), _ => 2000, path);
        var seat = new SeatInfo { AccountName = "user", Status = SeatStatus.Ready,
            SessionId = 42, VramBudgetMb = 3072 };
        gpu.RequireFits([seat], 3072);
        var fault = Assert.Throws<AdminFault>(() => gpu.RequireFits([seat], 4096));
        Assert.Equal("insufficient_vram", fault.Code);
        Assert.Contains("3072 Mo disponibles, 4096 Mo demandés", fault.Message);
        Assert.Equal(2000, gpu.Usage(42));
        seat.VramUsedMb = gpu.Usage(42);
        Assert.False(seat.VramOverBudget);
        gpu.SetReserve(4096);
        Assert.Equal(4096, new GpuBudgetService(() => ("Test GPU", 8192), _ => null, path).HostReserveMb);
        Directory.Delete(Path.GetDirectoryName(path)!, true);
    }

    [Fact]
    public void MissingUsageAndUnknownCapacityFailClosed()
    {
        var path = Path.Combine(Path.GetTempPath(), Guid.NewGuid().ToString("N"), "gpu.json");
        var gpu = new GpuBudgetService(() => ("", 0), _ => throw new IOException(), path);
        Assert.Null(gpu.Usage(4));
        Assert.Throws<AdminFault>(() => gpu.RequireFits([], 2048));
    }

    private static async Task<JsonElement> Dispatch(string method, string parameters,
        Func<string, JsonElement, Task<object?>>? execute = null, IReadOnlySet<string>? extra = null)
    {
        var bytes = Encoding.UTF8.GetBytes($"{{\"id\":1,\"method\":\"{method}\",\"params\":{parameters}}}");
        return JsonSerializer.SerializeToElement(await AdminDispatcher.DispatchAsync(bytes,
            execute ?? ((_, _) => throw new Exception("Invalid parameters reached service")), extra));
    }

    [Theory]
    [InlineData("seat.create", "{\"accountName\":\"user\",\"graphicsProfile\":\"Custom\",\"width\":1600}")]
    [InlineData("seat.create", "{\"accountName\":\"user\",\"graphicsProfile\":\"Quality\",\"fps\":90}")]
    [InlineData("seat.create", "{\"accountName\":\"user\",\"vramBudgetMb\":-1}")]
    [InlineData("gpu.budget.setReserve", "{\"hostReserveMb\":-1}")]
    [InlineData("gpu.budget", "{\"seatId\":\"bad\"}")]
    [InlineData("joinRequest.accept", "{\"requestId\":\"00000000-0000-0000-0000-000000000002\",\"seatId\":\"00000000-0000-0000-0000-000000000001\",\"graphicsProfile\":\"bogus\"}")]
    public async Task BadGraphicsParamsDoNotExecute(string method, string parameters)
    {
        var response = await Dispatch(method, parameters);
        Assert.Equal("invalid_params", response.GetProperty("error").GetProperty("code").GetString());
    }

    [Fact]
    public async Task CustomGraphicsAndExtraHandlerDispatch()
    {
        var create = await Dispatch("seat.create", "{\"accountName\":\"user\",\"graphicsProfile\":\"Custom\",\"width\":1600,\"height\":900,\"fps\":90,\"nvencPreset\":\"Latency\",\"vramBudgetMb\":2300}",
            (_, p) => Task.FromResult<object?>(new { profile = p.GetProperty("graphicsProfile").GetString() }));
        Assert.Equal("Custom", create.GetProperty("result").GetProperty("profile").GetString());
        var handler = new FakeHandler();
        var result = await Dispatch("library.test", "{\"gameId\":\"steam:1\"}",
            (method, p) => handler.HandleAsync(method, p, CancellationToken.None).ContinueWith<object?>(t => t.Result),
            handler.Methods.ToHashSet());
        Assert.Equal("steam:1", result.GetProperty("result").GetProperty("gameId").GetString());
        Assert.Equal("library.test", handler.Seen);
        var unknown = await Dispatch("library.test", "{}", extra: new HashSet<string>());
        Assert.Equal("unknown_method", unknown.GetProperty("error").GetProperty("code").GetString());
    }

    private sealed class FakeHandler : IAdminMethodHandler
    {
        public IReadOnlyCollection<string> Methods => ["library.test"];
        public string? Seen { get; private set; }
        public Task<object> HandleAsync(string method, JsonElement p, CancellationToken ct)
        {
            Seen = method;
            return Task.FromResult<object>(new { gameId = p.GetProperty("gameId").GetString() });
        }
    }
}
