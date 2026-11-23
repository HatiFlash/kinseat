using System.Text;
using System.Text.Json;
using Microsoft.AspNetCore.Http;
using Microsoft.Extensions.Logging.Abstractions;
using MultiSeat.Service.Ipc;
using MultiSeat.Shared.Models;
using Xunit;

namespace MultiSeat.Tests.Ipc;

public class AdminV11Tests
{
    private sealed class FakeTimeProvider : TimeProvider
    {
        private long _ticks;
        private DateTimeOffset _utc = new(2026, 11, 13, 12, 0, 0, TimeSpan.Zero);
        public override long TimestampFrequency => TimeSpan.TicksPerSecond;
        public override long GetTimestamp() => _ticks;
        public override DateTimeOffset GetUtcNow() => _utc;
        public void Advance(TimeSpan duration) { _ticks += duration.Ticks; _utc += duration; }
    }

    private sealed class Channel : IAgentChannel
    {
        public event Action<Guid, JsonElement>? Received;
        public event Action<Guid>? Connected;
        public event Action<Guid>? Disconnected;
        public bool Available { get; set; } = true;
        public bool IsConnected(Guid seatId) => Available;
        public void Connect(Guid seatId) { Available = true; Connected?.Invoke(seatId); }
        public void Disconnect(Guid seatId) { Available = false; Disconnected?.Invoke(seatId); }
        public readonly List<JsonElement> Sent = [];
        public Task SendAsync(Guid id, object message, CancellationToken ct = default)
        {
            if (!Available) throw new IOException("Seat agent unavailable");
            Sent.Add(JsonSerializer.SerializeToElement(message));
            return Task.CompletedTask;
        }
        public void Emit(Guid id, object message) => Received?.Invoke(id, JsonSerializer.SerializeToElement(message));
    }

    private static SeatInfo Seat() => new() { AccountName = "Test", Status = SeatStatus.Ready };
    private static JsonElement Shape(object value) => JsonSerializer.SerializeToElement(value, new JsonSerializerOptions(JsonSerializerDefaults.Web)
        { Converters = { new System.Text.Json.Serialization.JsonStringEnumConverter() } });
    private static async Task<JsonElement> Dispatch(string method, string parameters, Func<string, JsonElement, Task<object?>> handler) =>
        Shape(await AdminDispatcher.DispatchAsync(Encoding.UTF8.GetBytes(
            $"{{\"id\":3,\"method\":\"{method}\",\"params\":{parameters}}}"), handler));

    [Theory]
    [InlineData("seat.notice.schedule", "{\"seatId\":\"bad\",\"minutes\":1,\"message\":\"OK\"}")]
    [InlineData("seat.notice.schedule", "{\"seatId\":\"00000000-0000-0000-0000-000000000001\",\"minutes\":121,\"message\":\"OK\"}")]
    [InlineData("seat.notice.schedule", "{\"seatId\":\"00000000-0000-0000-0000-000000000001\",\"minutes\":1,\"message\":\"\"}")]
    [InlineData("seat.action", "{\"seatId\":\"00000000-0000-0000-0000-000000000001\",\"action\":\"stop_game\"}")]
    [InlineData("seat.preview.start", "{\"seatId\":\"00000000-0000-0000-0000-000000000001\",\"extra\":1}")]
    [InlineData("notices.list", "{\"seatId\":\"00000000-0000-0000-0000-000000000001\"}")]
    public async Task InvalidParamsNeverExecute(string method, string parameters)
    {
        var response = await Dispatch(method, parameters, (_, _) => throw new Exception("Executed"));
        Assert.Equal("invalid_params", response.GetProperty("error").GetProperty("code").GetString());
        Assert.Equal(3, response.GetProperty("id").GetInt32());
    }

    [Theory]
    [InlineData("seat.notice.cancel", "not_found")]
    [InlineData("seat.notice.schedule", "conflict")]
    [InlineData("seat.action", "too_late")]
    public async Task MapsDomainErrors(string method, string code)
    {
        var parameters = method == "seat.notice.schedule" ?
            "{\"seatId\":\"00000000-0000-0000-0000-000000000001\",\"minutes\":1,\"message\":\"OK\"}" :
            method == "seat.action" ?
            "{\"seatId\":\"00000000-0000-0000-0000-000000000001\",\"action\":\"close\"}" :
            "{\"seatId\":\"00000000-0000-0000-0000-000000000001\"}";
        var response = await Dispatch(method, parameters, (_, _) => throw new AdminFault(code, "Test"));
        Assert.Equal(code, response.GetProperty("error").GetProperty("code").GetString());
    }

    [Fact]
    public async Task DispatchesEveryNewMethod()
    {
        var seatId = Guid.NewGuid();
        foreach (var method in new[] { "seat.notice.schedule", "seat.notice.cancel", "notices.list",
            "seat.preview.start", "seat.preview.stop", "seat.preview.frame", "seat.action" })
        {
            var parameters = method switch
            {
                "notices.list" => "{}",
                "seat.notice.schedule" => $"{{\"seatId\":\"{seatId}\",\"minutes\":1,\"message\":\"OK\"}}",
                "seat.action" => $"{{\"seatId\":\"{seatId}\",\"action\":\"disconnect\"}}",
                _ => $"{{\"seatId\":\"{seatId}\"}}"
            };
            var called = false;
            var response = await Dispatch(method, parameters, (name, _) =>
            {
                Assert.Equal(method, name);
                called = true;
                return Task.FromResult<object?>(new { ok = true });
            });
            Assert.True(called);
            Assert.True(response.GetProperty("result").GetProperty("ok").GetBoolean());
        }
    }

    [Fact]
    public void PreviewSettingsDefaultAndRoundTrip()
    {
        var directory = Path.Combine(Path.GetTempPath(), $"kinseat-preview-{Guid.NewGuid():N}");
        try
        {
            var store = new PreviewSettingsStore(Path.Combine(directory, "settings.json"));
            Assert.True(store.ShowIndicator);
            store.SetShowIndicator(false);
            Assert.False(new PreviewSettingsStore(Path.Combine(directory, "settings.json")).ShowIndicator);
            store.SetShowIndicator(true);
            Assert.True(new PreviewSettingsStore(Path.Combine(directory, "settings.json")).ShowIndicator);
        }
        finally { if (Directory.Exists(directory)) Directory.Delete(directory, recursive: true); }
    }

    [Theory]
    [InlineData("{\"preview\":{\"showIndicator\":false}}", true)]
    [InlineData("{\"preview\":{\"showIndicator\":0}}", false)]
    [InlineData("{\"preview\":{\"showIndicator\":false,\"notice\":false}}", false)]
    public async Task SettingsSetStrictlyValidatesPayload(string parameters, bool valid)
    {
        var called = false;
        var result = await Dispatch("settings.set", parameters, (_, _) =>
        {
            called = true;
            return Task.FromResult<object?>(new { preview = new { showIndicator = false } });
        });
        Assert.Equal(valid, called);
        if (!valid) Assert.Equal("invalid_params", result.GetProperty("error").GetProperty("code").GetString());
    }

    [Fact]
    public void AdminPipeAclRestrictsSettingsToLocalAdminsAndSystem()
    {
        if (!OperatingSystem.IsWindows()) return;
        // Aucun compte autorisé : seuls SYSTEM et les Administrateurs élevés ouvrent le canal.
        var security = AdminPipeServer.CreateSecurity([]);
        var rules = security.GetAccessRules(true, false,
            typeof(System.Security.Principal.SecurityIdentifier))
            .Cast<System.IO.Pipes.PipeAccessRule>().ToArray();
        Assert.Equal(3, rules.Length);
        Assert.Contains(rules, r => r.IdentityReference.Value == "S-1-5-2" &&
            r.AccessControlType == System.Security.AccessControl.AccessControlType.Deny);
        Assert.Equal(new[] { "S-1-5-18", "S-1-5-32-544" }, rules
            .Where(r => r.AccessControlType == System.Security.AccessControl.AccessControlType.Allow)
            .Select(r => r.IdentityReference.Value).OrderBy(s => s).ToArray());
    }

    [Fact]
    public async Task DisplayedClosesAtDeadline()
    {
        var clock = new FakeTimeProvider(); var channel = new Channel(); var seat = Seat(); var closes = 0;
        var scheduler = new NoticeScheduler(clock, channel, (_, _) => { closes++; return Task.CompletedTask; },
            NullLogger<NoticeScheduler>.Instance);
        var notice = scheduler.Schedule(seat, 1, "Fermer");
        await scheduler.TickAsync();
        Assert.Equal("notice.show", channel.Sent.Single().GetProperty("type").GetString());
        channel.Emit(seat.Id, new { type = "notice.displayed", noticeId = notice.Id });
        clock.Advance(TimeSpan.FromSeconds(10)); await scheduler.TickAsync();
        Assert.Equal(2, channel.Sent.Count);
        clock.Advance(TimeSpan.FromSeconds(50)); await scheduler.TickAsync();
        Assert.Equal(1, closes);
        Assert.Equal(NoticePhase.Closed, notice.Phase);
        Assert.Equal(clock.GetUtcNow(), scheduler.LastHostClose(seat.Id));
    }

    [Fact]
    public async Task FailedTeardownIsNotReportedAsHostClosure()
    {
        var clock = new FakeTimeProvider(); var channel = new Channel(); var seat = Seat();
        var scheduler = new NoticeScheduler(clock, channel, (_, _) => throw new IOException("teardown failed"),
            NullLogger<NoticeScheduler>.Instance);
        var notice = scheduler.Schedule(seat, 1, "Fermer");
        await scheduler.TickAsync();
        channel.Emit(seat.Id, new { type = "notice.displayed", noticeId = notice.Id });
        clock.Advance(TimeSpan.FromMinutes(1));
        await scheduler.TickAsync();
        Assert.Equal(NoticePhase.Failed, notice.Phase);
        Assert.Null(scheduler.LastHostClose(seat.Id));
    }

    [Fact]
    public async Task MissingAckFailsWithoutClosing()
    {
        var clock = new FakeTimeProvider(); var channel = new Channel(); var seat = Seat(); var closes = 0;
        var scheduler = new NoticeScheduler(clock, channel, (_, _) => { closes++; return Task.CompletedTask; },
            NullLogger<NoticeScheduler>.Instance);
        var notice = scheduler.Schedule(seat, 1, "Fermer");
        await scheduler.TickAsync();
        clock.Advance(TimeSpan.FromSeconds(15)); await scheduler.TickAsync();
        Assert.Equal(NoticePhase.Failed, notice.Phase);
        Assert.Null(scheduler.LastHostClose(seat.Id));
        Assert.Equal(0, closes);
    }

    [Fact]
    public async Task CancelPreventsCloseAndAfterClosingIsTooLate()
    {
        var clock = new FakeTimeProvider(); var channel = new Channel(); var seat = Seat(); var closes = 0;
        var scheduler = new NoticeScheduler(clock, channel, (_, _) => { closes++; return Task.CompletedTask; },
            NullLogger<NoticeScheduler>.Instance);
        var notice = scheduler.Schedule(seat, 1, "Fermer");
        await scheduler.TickAsync();
        channel.Emit(seat.Id, new { type = "notice.displayed", noticeId = notice.Id });
        Assert.Equal(NoticePhase.Cancelled, scheduler.Cancel(seat.Id).Phase);
        clock.Advance(TimeSpan.FromMinutes(2)); await scheduler.TickAsync();
        Assert.Equal(0, closes);
        var second = scheduler.Schedule(seat, 1, "Fermer");
        await scheduler.TickAsync();
        channel.Emit(seat.Id, new { type = "notice.displayed", noticeId = second.Id });
        clock.Advance(TimeSpan.FromMinutes(1)); await scheduler.TickAsync();
        Assert.Equal("too_late", Assert.Throws<AdminFault>(() => scheduler.Cancel(seat.Id)).Code);
    }

    [Fact]
    public async Task NoticeWaitsForHelloBeforeStartingAckTimeout()
    {
        var clock = new FakeTimeProvider(); var channel = new Channel { Available = false };
        var seat = Seat(); var closes = 0;
        var scheduler = new NoticeScheduler(clock, channel, (_, _) => { closes++; return Task.CompletedTask; },
            NullLogger<NoticeScheduler>.Instance);
        var notice = scheduler.Schedule(seat, 2, "Attendre");
        await scheduler.TickAsync();
        clock.Advance(TimeSpan.FromSeconds(30)); await scheduler.TickAsync();
        Assert.Equal(NoticePhase.Pending, notice.Phase);
        Assert.Empty(channel.Sent);
        channel.Connect(seat.Id);
        Assert.Equal(NoticePhase.Sending, notice.Phase);
        Assert.Single(channel.Sent);
        channel.Emit(seat.Id, new { type = "notice.displayed", noticeId = notice.Id });
        Assert.Equal(NoticePhase.Displayed, notice.Phase);
        clock.Advance(TimeSpan.FromSeconds(90)); await scheduler.TickAsync();
        Assert.Equal(1, closes);
    }

    [Fact]
    public async Task NeverConnectedFailsWithoutClosing()
    {
        var clock = new FakeTimeProvider(); var channel = new Channel { Available = false };
        var seat = Seat(); var closes = 0;
        var scheduler = new NoticeScheduler(clock, channel, (_, _) => { closes++; return Task.CompletedTask; },
            NullLogger<NoticeScheduler>.Instance, TimeSpan.FromSeconds(20));
        var notice = scheduler.Schedule(seat, 1, "Attendre");
        clock.Advance(TimeSpan.FromSeconds(19)); await scheduler.TickAsync();
        Assert.Equal(NoticePhase.Pending, notice.Phase);
        clock.Advance(TimeSpan.FromSeconds(1)); await scheduler.TickAsync();
        Assert.Equal(NoticePhase.Failed, notice.Phase);
        Assert.Contains("agent", notice.Detail!, StringComparison.OrdinalIgnoreCase);
        clock.Advance(TimeSpan.FromMinutes(2)); await scheduler.TickAsync();
        Assert.Equal(0, closes);
    }

    [Fact]
    public async Task DisconnectReissuesUnacknowledgedAndDisplayedNoticesBeforeClosing()
    {
        var clock = new FakeTimeProvider(); var channel = new Channel(); var seat = Seat(); var closes = 0;
        var scheduler = new NoticeScheduler(clock, channel, (_, _) => { closes++; return Task.CompletedTask; },
            NullLogger<NoticeScheduler>.Instance);
        var notice = scheduler.Schedule(seat, 1, "Fermer");
        await scheduler.TickAsync();
        Assert.Equal(NoticePhase.Sending, notice.Phase);
        channel.Disconnect(seat.Id);
        Assert.Equal(NoticePhase.Pending, notice.Phase);
        clock.Advance(TimeSpan.FromSeconds(16));
        await scheduler.TickAsync();
        Assert.Equal(NoticePhase.Pending, notice.Phase);
        channel.Connect(seat.Id);
        await scheduler.TickAsync();
        Assert.Equal(2, channel.Sent.Count);
        channel.Emit(seat.Id, new { type = "notice.displayed", noticeId = notice.Id });
        Assert.Equal(NoticePhase.Displayed, notice.Phase);
        channel.Disconnect(seat.Id);
        clock.Advance(TimeSpan.FromSeconds(44));
        await scheduler.TickAsync();
        Assert.Equal(0, closes);
        channel.Connect(seat.Id);
        await scheduler.TickAsync();
        Assert.Equal(3, channel.Sent.Count);
        channel.Emit(seat.Id, new { type = "notice.displayed", noticeId = notice.Id });
        await scheduler.TickAsync();
        Assert.Equal(1, closes);
    }

    [Theory]
    [InlineData(-1, false)]
    [InlineData(0, true)]
    [InlineData(120, true)]
    [InlineData(121, false)]
    public void HostClosureWindow(int seconds, bool expected)
    {
        var now = new DateTimeOffset(2026, 11, 20, 12, 0, 0, TimeSpan.Zero);
        var closed = now.AddSeconds(-seconds);
        Assert.Equal(expected ? closed : null, JoinRequestListener.RecentHostClose(closed, now));
        Assert.Null(JoinRequestListener.RecentHostClose(null, now));
    }

    [Fact]
    public async Task SessionStatusRequiresJoinTokenAndOnlyRevealsRecentClosure()
    {
        var clock = new FakeTimeProvider(); var channel = new Channel(); var seat = Seat();
        var store = new JoinRequestStore(clock);
        var scheduler = new NoticeScheduler(clock, channel, (_, _) => Task.CompletedTask,
            NullLogger<NoticeScheduler>.Instance);
        var request = store.Submit("Alice", "PC-Alice", "device-0001", "192.168.1.20");

        async Task<(int Code, JsonElement? Body)> Query(Guid id, string? token)
        {
            var ctx = new DefaultHttpContext();
            ctx.Response.Body = new MemoryStream();
            if (token is not null) ctx.Request.Headers["X-Kinseat-Token"] = token;
            await JoinRequestListener.SessionAsync(ctx, id, store, scheduler, clock.GetUtcNow());
            ctx.Response.Body.Position = 0;
            if (ctx.Response.Body.Length == 0) return (ctx.Response.StatusCode, null);
            using var doc = await JsonDocument.ParseAsync(ctx.Response.Body);
            return (ctx.Response.StatusCode, doc.RootElement.Clone());
        }

        Assert.Equal(404, (await Query(request.RequestId, null)).Code);
        Assert.Equal(404, (await Query(request.RequestId, "wrong")).Code);
        Assert.Equal(404, (await Query(Guid.NewGuid(), request.Token)).Code);
        var pending = await Query(request.RequestId, request.Token);
        Assert.Equal(200, pending.Code);
        Assert.False(pending.Body!.Value.GetProperty("closedByHost").GetBoolean());
        Assert.Equal(JsonValueKind.Null, pending.Body.Value.GetProperty("closedAtUtc").ValueKind);
        Assert.Equal(2, pending.Body.Value.EnumerateObject().Count());

        Assert.Equal(JoinTransition.Ok, store.Accept(request.RequestId, seat.Id));
        Assert.False((await Query(request.RequestId, request.Token)).Body!.Value.GetProperty("closedByHost").GetBoolean());
        var notice = scheduler.Schedule(seat, 1, "Fermer");
        await scheduler.TickAsync();
        channel.Emit(seat.Id, new { type = "notice.displayed", noticeId = notice.Id });
        clock.Advance(TimeSpan.FromMinutes(1));
        await scheduler.TickAsync();
        var closed = await Query(request.RequestId, request.Token);
        Assert.True(closed.Body!.Value.GetProperty("closedByHost").GetBoolean());
        Assert.Equal(clock.GetUtcNow(), closed.Body.Value.GetProperty("closedAtUtc").GetDateTimeOffset());
        Assert.True(store.MarkPaired(request.RequestId));
        Assert.True((await Query(request.RequestId, request.Token)).Body!.Value.GetProperty("closedByHost").GetBoolean());
        clock.Advance(TimeSpan.FromSeconds(121));
        var stale = await Query(request.RequestId, request.Token);
        Assert.False(stale.Body!.Value.GetProperty("closedByHost").GetBoolean());
        Assert.Equal(JsonValueKind.Null, stale.Body.Value.GetProperty("closedAtUtc").ValueKind);
    }

    [Fact]
    public async Task AgentFramingMultipleLargeFramesAndMalformedFrameIsNotEof()
    {
        using var stream = new MemoryStream();
        await AgentFraming.WriteAsync(stream, new { type = "hello" }, default);
        for (var i = 0; i < 3; i++)
            await AgentFraming.WriteAsync(stream,
                new { type = "preview.frame", jpegBase64 = new string((char)('a' + i), 150 * 1024) }, default);
        stream.Position = 0;
        Assert.Equal("hello", (await AgentFraming.ReadAsync(stream, default))!.Value.GetProperty("type").GetString());
        for (var i = 0; i < 3; i++)
            Assert.Equal(new string((char)('a' + i), 150 * 1024),
                (await AgentFraming.ReadAsync(stream, default))!.Value.GetProperty("jpegBase64").GetString());
        Assert.Null(await AgentFraming.ReadAsync(stream, default));
        using var malformed = new MemoryStream(Encoding.UTF8.GetBytes("{bad}\n{\"type\":\"hello\"}\n"));
        await Assert.ThrowsAsync<InvalidDataException>(() => AgentFraming.ReadAsync(malformed, default));
        Assert.Equal("hello", (await AgentFraming.ReadAsync(malformed, default))!.Value.GetProperty("type").GetString());
    }

    [Fact]
    public async Task AgentFramingMultipleFramesAndTruncation()
    {
        using var stream = new MemoryStream();
        await AgentFraming.WriteAsync(stream, new { type = "hello" }, default);
        await AgentFraming.WriteAsync(stream, new { type = "preview.frame", jpegBase64 = new string('x', 64_000) }, default);
        stream.Position = 0;
        Assert.Equal("hello", (await AgentFraming.ReadAsync(stream, default))!.Value.GetProperty("type").GetString());
        Assert.Equal(64_000, (await AgentFraming.ReadAsync(stream, default))!.Value.GetProperty("jpegBase64").GetString()!.Length);
        Assert.Null(await AgentFraming.ReadAsync(stream, default));
        using var truncated = new MemoryStream(Encoding.UTF8.GetBytes("{\"type\":\"hello\"}"));
        await Assert.ThrowsAsync<EndOfStreamException>(() => AgentFraming.ReadAsync(truncated, default));
    }

    [Fact]
    public async Task PreviewExpiresAndStopsWhenNoObserver()
    {
        var clock = new FakeTimeProvider(); var channel = new Channel(); var seat = Seat();
        var broker = new PreviewBroker(channel, clock, NullLogger<PreviewBroker>.Instance);
        await broker.StartAsync(seat.Id, default);
        channel.Emit(seat.Id, new { type = "preview.frame", jpegBase64 = "AA==", width = 1, height = 1,
            capturedAtUtc = clock.GetUtcNow() });
        Assert.True(Shape(broker.Frame(seat.Id)).GetProperty("live").GetBoolean());
        clock.Advance(TimeSpan.FromSeconds(4));
        Assert.False(Shape(broker.Frame(seat.Id)).GetProperty("live").GetBoolean());
        clock.Advance(TimeSpan.FromSeconds(11)); await broker.TickAsync();
        Assert.False(Shape(broker.Frame(seat.Id)).GetProperty("active").GetBoolean());
        Assert.Equal("preview.stop", channel.Sent.Last().GetProperty("type").GetString());
    }

    [Fact]
    public async Task AgentFramingRoundTripAndBounds()
    {
        using var stream = new MemoryStream();
        await AgentFraming.WriteAsync(stream, new { type = "notice.displayed", noticeId = Guid.Empty }, default);
        stream.Position = 0;
        Assert.Equal("notice.displayed", (await AgentFraming.ReadAsync(stream, default))!.Value.GetProperty("type").GetString());
        using var oversized = new MemoryStream(Enumerable.Repeat((byte)'a', AgentFraming.MaxLineBytes).ToArray());
        await Assert.ThrowsAsync<InvalidDataException>(() => AgentFraming.ReadAsync(oversized, default));
    }
}
