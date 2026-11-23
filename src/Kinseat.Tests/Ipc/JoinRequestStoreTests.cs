using System.Text;
using System.Text.Json;
using MultiSeat.Service.Ipc;
using Xunit;

namespace MultiSeat.Tests.Ipc;

public class JoinRequestStoreTests
{
    private sealed class ManualClock : TimeProvider
    {
        private DateTimeOffset _utc = new(2026, 11, 17, 12, 0, 0, TimeSpan.Zero);
        public override DateTimeOffset GetUtcNow() => _utc;
        public void Advance(TimeSpan duration) => _utc += duration;
    }

    private static (JoinRequestStore Store, ManualClock Clock) Create()
    {
        var clock = new ManualClock();
        return (new JoinRequestStore(clock), clock);
    }

    private static JoinSubmitResult Submit(JoinRequestStore store, string address = "192.168.1.20",
        string deviceId = "device-0001", string pseudo = "Alice") =>
        store.Submit(pseudo, "PC-Alice", deviceId, address);

    [Fact]
    public void SubmitReturnsIdAnd32ByteBase64UrlToken()
    {
        var (store, _) = Create();
        var result = Submit(store);
        Assert.Equal(JoinSubmitOutcome.Created, result.Outcome);
        Assert.NotEqual(Guid.Empty, result.RequestId);
        Assert.Equal(43, result.Token!.Length);
        Assert.DoesNotContain('+', result.Token);
        Assert.DoesNotContain('/', result.Token);
        Assert.DoesNotContain('=', result.Token);
        Assert.NotEqual(result.Token, Submit(store, "192.168.1.21").Token);
    }

    [Fact]
    public void RightTokenReadsPendingRequest()
    {
        var (store, _) = Create();
        var result = Submit(store);
        var snapshot = store.Get(result.RequestId, result.Token);
        Assert.NotNull(snapshot);
        Assert.Equal(JoinStatus.Pending, snapshot!.Status);
        Assert.Equal("Alice", snapshot.Pseudo);
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("wrong")]
    public void WrongOrMissingTokenLooksLikeUnknownRequest(string? token)
    {
        var (store, _) = Create();
        var result = Submit(store);
        Assert.Null(store.Get(result.RequestId, token));
        Assert.Null(store.Get(Guid.NewGuid(), result.Token));
        Assert.Null(store.BeginPin(result.RequestId, token));
    }

    [Fact]
    public void TokenOfAnotherRequestIsRefused()
    {
        var (store, _) = Create();
        var first = Submit(store, "192.168.1.20");
        var second = Submit(store, "192.168.1.21");
        Assert.Null(store.Get(first.RequestId, second.Token));
    }

    [Fact]
    public void NewRequestOfSameAddressAndDeviceReplacesOldOne()
    {
        var (store, _) = Create();
        var first = Submit(store);
        var second = Submit(store);
        Assert.Equal(JoinSubmitOutcome.Created, second.Outcome);
        Assert.Null(store.Get(first.RequestId, first.Token));
        Assert.Single(store.List());
    }

    [Fact]
    public void SecondPendingRequestFromSameAddressWithOtherDeviceIsRateLimited()
    {
        var (store, _) = Create();
        Submit(store);
        var other = Submit(store, deviceId: "device-0002");
        Assert.Equal(JoinSubmitOutcome.RateLimited, other.Outcome);
        Assert.Null(other.Token);
        Assert.Single(store.List());
    }

    [Fact]
    public void AtMostEightPendingRequestsInTotal()
    {
        var (store, _) = Create();
        for (var i = 0; i < JoinRequestStore.MaxPending; i++)
            Assert.Equal(JoinSubmitOutcome.Created, Submit(store, $"192.168.1.{10 + i}").Outcome);
        Assert.Equal(JoinSubmitOutcome.RateLimited, Submit(store, "192.168.1.99").Outcome);
        Assert.Equal(8, store.List().Count);
    }

    [Fact]
    public void ReplacementDoesNotConsumeASlotWhenFull()
    {
        var (store, _) = Create();
        for (var i = 0; i < JoinRequestStore.MaxPending; i++) Submit(store, $"192.168.1.{10 + i}");
        Assert.Equal(JoinSubmitOutcome.Created, Submit(store, "192.168.1.10").Outcome);
        Assert.Equal(8, store.List().Count);
    }

    [Fact]
    public void RefusedRequestFreesItsSlot()
    {
        var (store, _) = Create();
        for (var i = 0; i < JoinRequestStore.MaxPending; i++) Submit(store, $"192.168.1.{10 + i}");
        var first = store.List()[0];
        Assert.Equal(JoinTransition.Ok, store.Refuse(first.Id));
        Assert.Equal(JoinSubmitOutcome.Created, Submit(store, "192.168.1.99").Outcome);
    }

    [Fact]
    public void PendingRequestExpiresAfterTenMinutes()
    {
        var (store, clock) = Create();
        var result = Submit(store);
        clock.Advance(TimeSpan.FromMinutes(9) + TimeSpan.FromSeconds(59));
        Assert.Equal(JoinStatus.Pending, store.Get(result.RequestId, result.Token)!.Status);
        clock.Advance(TimeSpan.FromSeconds(1));
        Assert.Equal(JoinStatus.Expired, store.Get(result.RequestId, result.Token)!.Status);
    }

    [Fact]
    public void ExpiredRequestCannotBeAcceptedOrRefused()
    {
        var (store, clock) = Create();
        var result = Submit(store);
        clock.Advance(JoinRequestStore.Lifetime);
        Assert.Equal(JoinTransition.WrongState, store.Accept(result.RequestId, Guid.NewGuid()));
        Assert.Equal(JoinTransition.WrongState, store.Refuse(result.RequestId));
    }

    [Fact]
    public void ExpiredRequestFreesItsSlotAndIsForgottenAfterRetention()
    {
        var (store, clock) = Create();
        var result = Submit(store);
        clock.Advance(JoinRequestStore.Lifetime);
        Assert.Equal(JoinSubmitOutcome.Created, Submit(store, deviceId: "device-0002").Outcome);
        clock.Advance(JoinRequestStore.Retention);
        Assert.Null(store.Get(result.RequestId, result.Token));
    }

    [Fact]
    public void AcceptMovesPendingToAcceptedWithSeat()
    {
        var (store, _) = Create();
        var result = Submit(store);
        var seat = Guid.NewGuid();
        Assert.Equal(JoinTransition.Ok, store.Accept(result.RequestId, seat));
        var snapshot = store.Get(result.RequestId, result.Token)!;
        Assert.Equal(JoinStatus.Accepted, snapshot.Status);
        Assert.Equal(seat, snapshot.SeatId);
    }

    [Fact]
    public void RefuseMovesPendingToRefused()
    {
        var (store, _) = Create();
        var result = Submit(store);
        Assert.Equal(JoinTransition.Ok, store.Refuse(result.RequestId));
        Assert.Equal(JoinStatus.Refused, store.Get(result.RequestId, result.Token)!.Status);
    }

    [Fact]
    public void UnknownRequestIsNotFound()
    {
        var (store, _) = Create();
        Assert.Equal(JoinTransition.NotFound, store.Accept(Guid.NewGuid(), Guid.NewGuid()));
        Assert.Equal(JoinTransition.NotFound, store.Refuse(Guid.NewGuid()));
    }

    [Fact]
    public void DecidedRequestCannotBeDecidedAgain()
    {
        var (store, _) = Create();
        var result = Submit(store);
        store.Refuse(result.RequestId);
        Assert.Equal(JoinTransition.WrongState, store.Accept(result.RequestId, Guid.NewGuid()));
        Assert.Equal(JoinTransition.WrongState, store.Refuse(result.RequestId));
        Assert.Equal(JoinStatus.Refused, store.Get(result.RequestId, result.Token)!.Status);
    }

    [Fact]
    public void PinIsOnlyAcceptedInAcceptedState()
    {
        var (store, _) = Create();
        var result = Submit(store);
        Assert.Null(store.BeginPin(result.RequestId, result.Token));
        store.Accept(result.RequestId, Guid.NewGuid());
        Assert.NotNull(store.BeginPin(result.RequestId, result.Token));
    }

    [Fact]
    public void OnlyOnePinSubmissionCanBeInFlight()
    {
        var (store, _) = Create();
        var result = Submit(store);
        store.Accept(result.RequestId, Guid.NewGuid());
        Assert.NotNull(store.BeginPin(result.RequestId, result.Token));
        Assert.Null(store.BeginPin(result.RequestId, result.Token));
    }

    [Fact]
    public void PairedAndFailedAreFinalAndOnlyFollowAccepted()
    {
        var (store, _) = Create();
        var pending = Submit(store, "192.168.1.20");
        Assert.False(store.MarkPaired(pending.RequestId));
        Assert.False(store.MarkFailed(pending.RequestId, "non"));

        var paired = Submit(store, "192.168.1.21");
        store.Accept(paired.RequestId, Guid.NewGuid());
        Assert.True(store.MarkPaired(paired.RequestId));
        Assert.Equal(JoinStatus.Paired, store.Get(paired.RequestId, paired.Token)!.Status);
        Assert.False(store.MarkFailed(paired.RequestId, "non"));

        var failed = Submit(store, "192.168.1.22");
        store.Accept(failed.RequestId, Guid.NewGuid());
        store.BeginPin(failed.RequestId, failed.Token);
        Assert.True(store.MarkFailed(failed.RequestId, "Code refusé"));
        var snapshot = store.Get(failed.RequestId, failed.Token)!;
        Assert.Equal(JoinStatus.Failed, snapshot.Status);
        Assert.Equal("Code refusé", snapshot.Message);
        Assert.Null(store.BeginPin(failed.RequestId, failed.Token));
    }

    [Fact]
    public void AcceptedRequestExpiresIfNeverPaired()
    {
        var (store, clock) = Create();
        var result = Submit(store);
        store.Accept(result.RequestId, Guid.NewGuid());
        clock.Advance(JoinRequestStore.Lifetime);
        Assert.Equal(JoinStatus.Expired, store.Get(result.RequestId, result.Token)!.Status);
    }

    [Fact]
    public void ListPutsPendingFirstThenMostRecentOthers()
    {
        var (store, clock) = Create();
        var a = Submit(store, "192.168.1.20");
        clock.Advance(TimeSpan.FromSeconds(1));
        var b = Submit(store, "192.168.1.21");
        clock.Advance(TimeSpan.FromSeconds(1));
        var c = Submit(store, "192.168.1.22");
        store.Refuse(a.RequestId);
        store.Refuse(b.RequestId);
        var list = store.List();
        Assert.Equal(new[] { c.RequestId, b.RequestId, a.RequestId }, list.Select(r => r.Id).ToArray());
        Assert.Equal(JoinStatus.Pending, list[0].Status);
    }

    [Theory]
    [InlineData("Al", true)]
    [InlineData("Alice du 2e-étage_x.y", true)]
    [InlineData("A", false)]
    [InlineData("", false)]
    [InlineData(" Alice", false)]
    [InlineData("Alice!", false)]
    [InlineData("Alice\n", false)]
    [InlineData("123456789012345678901234", true)]
    [InlineData("1234567890123456789012345", false)]
    public void PseudoRules(string value, bool valid) => Assert.Equal(valid, JoinRequestStore.IsValidPseudo(value));

    [Theory]
    [InlineData("device-0001", true)]
    [InlineData("1234567", false)]
    [InlineData("device 0001", false)]
    [InlineData("device_0001", false)]
    [InlineData("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", false)]
    public void DeviceIdRules(string value, bool valid) => Assert.Equal(valid, JoinRequestStore.IsValidDeviceId(value));

    [Theory]
    [InlineData("PC-Alice", true)]
    [InlineData("", false)]
    [InlineData("a\tb", false)]
    [InlineData("1234567890123456789012345678901234567890", true)]
    [InlineData("12345678901234567890123456789012345678901", false)]
    public void DeviceRules(string value, bool valid) => Assert.Equal(valid, JoinRequestStore.IsValidDevice(value));

    // ── Admin pipe contract ──────────────────────────────────────────────────

    private static async Task<JsonElement> Dispatch(string method, string parameters,
        Func<string, JsonElement, Task<object?>>? execute = null)
    {
        var line = Encoding.UTF8.GetBytes($"{{\"id\":1,\"method\":\"{method}\",\"params\":{parameters}}}");
        var result = await AdminDispatcher.DispatchAsync(line, execute ?? ((_, _) =>
            throw new Exception("Invalid parameters reached service")));
        return JsonSerializer.SerializeToElement(result);
    }

    private const string Request = "00000000-0000-0000-0000-000000000002";
    private const string Seat = "00000000-0000-0000-0000-000000000001";

    [Theory]
    [InlineData("joinRequests.list", "{\"x\":1}")]
    [InlineData("joinRequest.accept", "{\"requestId\":\"" + Request + "\"}")]
    [InlineData("joinRequest.accept", "{\"requestId\":\"bad\",\"seatId\":\"" + Seat + "\"}")]
    [InlineData("joinRequest.accept", "{\"requestId\":\"" + Request + "\",\"seatId\":\"bad\"}")]
    [InlineData("joinRequest.refuse", "{\"requestId\":\"bad\"}")]
    [InlineData("joinRequest.refuse", "{\"seatId\":\"" + Seat + "\"}")]
    [InlineData("joinRequest.refuse", "{\"requestId\":\"" + Request + "\",\"seatId\":\"" + Seat + "\"}")]
    public async Task InvalidJoinParamsNeverExecute(string method, string parameters)
    {
        var response = await Dispatch(method, parameters);
        Assert.Equal("invalid_params", response.GetProperty("error").GetProperty("code").GetString());
    }

    [Theory]
    [InlineData("joinRequests.list", "{}")]
    [InlineData("joinRequest.accept", "{\"requestId\":\"" + Request + "\",\"seatId\":\"" + Seat + "\"}")]
    [InlineData("joinRequest.refuse", "{\"requestId\":\"" + Request + "\"}")]
    public async Task ValidJoinMethodsReachTheService(string method, string parameters)
    {
        string? seen = null;
        var response = await Dispatch(method, parameters, (m, _) => { seen = m; return Task.FromResult<object?>(new { ok = true }); });
        Assert.Equal(method, seen);
        Assert.True(response.TryGetProperty("result", out _));
    }
}
