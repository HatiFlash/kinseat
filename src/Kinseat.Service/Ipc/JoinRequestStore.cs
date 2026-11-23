using System.Security.Cryptography;
using System.Text;

namespace MultiSeat.Service.Ipc;

public enum JoinStatus { Pending, Refused, Accepted, Paired, Expired, Failed }

public enum JoinSubmitOutcome { Created, RateLimited }

public enum JoinTransition { Ok, NotFound, WrongState }

/// <summary>Immutable copy of a request, safe to hand out of the store's lock.</summary>
public sealed record JoinRequestSnapshot(
    Guid Id, string Pseudo, string Device, string DeviceId, string Address,
    DateTimeOffset CreatedAt, JoinStatus Status, string? Message, Guid? SeatId);

public sealed record JoinSubmitResult(JoinSubmitOutcome Outcome, Guid RequestId, string? Token);

/// <summary>
/// In-memory join requests sent by Kinseat clients on the LAN. Nothing is granted here: a request
/// only becomes Accepted through the admin pipe (an explicit click on the Host app).
/// The clock is injected so expiry can be tested without waiting.
/// </summary>
public sealed class JoinRequestStore(TimeProvider clock)
{
    internal const int MaxPending = 8;
    internal const int MaxEntries = 64;
    internal static readonly TimeSpan Lifetime = TimeSpan.FromMinutes(10);
    /// <summary>How long a finished request stays readable so the client can see its outcome.</summary>
    internal static readonly TimeSpan Retention = TimeSpan.FromHours(1);

    private sealed class Entry
    {
        public required Guid Id { get; init; }
        public required string Pseudo { get; init; }
        public required string Device { get; init; }
        public required string DeviceId { get; init; }
        public required string Address { get; init; }
        public required byte[] Token { get; init; }
        public required DateTimeOffset CreatedAt { get; init; }
        public DateTimeOffset ChangedAt { get; set; }
        public JoinStatus Status { get; set; } = JoinStatus.Pending;
        public string? Message { get; set; }
        public Guid? SeatId { get; set; }
        public bool PinInProgress { get; set; }
    }

    private readonly object _gate = new();
    private readonly List<Entry> _entries = [];

    // ── Input rules (docs/join-requests.md §1) ───────────────────────────────

    internal static bool IsValidPseudo(string? value) =>
        value is { Length: >= 2 and <= 24 } && !string.IsNullOrWhiteSpace(value) &&
        value == value.Trim() &&
        value.All(c => char.IsLetterOrDigit(c) || c is ' ' or '-' or '_' or '.');

    internal static bool IsValidDevice(string? value) =>
        value is { Length: >= 1 and <= 40 } && !string.IsNullOrWhiteSpace(value) &&
        !value.Any(char.IsControl);

    internal static bool IsValidDeviceId(string? value) =>
        value is { Length: >= 8 and <= 64 } &&
        value.All(c => c is (>= 'A' and <= 'Z') or (>= 'a' and <= 'z') or (>= '0' and <= '9') or '-');

    // ── Client side ─────────────────────────────────────────────────────────

    /// <summary>
    /// Registers a request. A new one replaces the pending request of the same address and
    /// deviceId; another pending request from the same address (other device) or a full queue
    /// is refused as rate limited. The inputs must already be validated.
    /// </summary>
    internal JoinSubmitResult Submit(string pseudo, string device, string deviceId, string address)
    {
        lock (_gate)
        {
            var now = clock.GetUtcNow();
            Sweep(now);
            _entries.RemoveAll(e => e.Status == JoinStatus.Pending && e.Address == address &&
                string.Equals(e.DeviceId, deviceId, StringComparison.Ordinal));
            if (_entries.Any(e => e.Status == JoinStatus.Pending && e.Address == address) ||
                _entries.Count(e => e.Status == JoinStatus.Pending) >= MaxPending)
                return new JoinSubmitResult(JoinSubmitOutcome.RateLimited, Guid.Empty, null);

            var tokenBytes = RandomNumberGenerator.GetBytes(32);
            var entry = new Entry
            {
                Id = Guid.NewGuid(), Pseudo = pseudo, Device = device, DeviceId = deviceId,
                Address = address, Token = tokenBytes, CreatedAt = now, ChangedAt = now
            };
            _entries.Add(entry);
            return new JoinSubmitResult(JoinSubmitOutcome.Created, entry.Id, Base64Url(tokenBytes));
        }
    }

    /// <summary>The request when id and token match; null otherwise (unknown id and wrong token look the same).</summary>
    internal JoinRequestSnapshot? Get(Guid id, string? token)
    {
        lock (_gate)
        {
            Sweep(clock.GetUtcNow());
            var entry = Authenticate(id, token);
            return entry is null ? null : Snapshot(entry);
        }
    }

    /// <summary>
    /// Authenticates the pin submission and claims it: only an Accepted request with no pin in
    /// flight yields the seat. The caller then reports <see cref="MarkPaired"/> or <see cref="MarkFailed"/>.
    /// </summary>
    internal JoinRequestSnapshot? BeginPin(Guid id, string? token)
    {
        lock (_gate)
        {
            Sweep(clock.GetUtcNow());
            var entry = Authenticate(id, token);
            if (entry is null || entry.Status != JoinStatus.Accepted || entry.SeatId is null || entry.PinInProgress)
                return null;
            entry.PinInProgress = true;
            return Snapshot(entry);
        }
    }

    internal bool MarkPaired(Guid id) => Finish(id, JoinStatus.Paired, null);

    internal bool MarkFailed(Guid id, string message) => Finish(id, JoinStatus.Failed, message);

    private bool Finish(Guid id, JoinStatus status, string? message)
    {
        lock (_gate)
        {
            var entry = _entries.FirstOrDefault(e => e.Id == id);
            if (entry is null || entry.Status != JoinStatus.Accepted) return false;
            entry.Status = status;
            entry.Message = message;
            entry.PinInProgress = false;
            entry.ChangedAt = clock.GetUtcNow();
            return true;
        }
    }

    // ── Host side (admin pipe) ──────────────────────────────────────────────

    /// <summary>Pending requests first (oldest first), then the others, most recent first.</summary>
    internal IReadOnlyList<JoinRequestSnapshot> List()
    {
        lock (_gate)
        {
            Sweep(clock.GetUtcNow());
            return _entries
                .OrderBy(e => e.Status == JoinStatus.Pending ? 0 : 1)
                .ThenBy(e => e.Status == JoinStatus.Pending ? e.CreatedAt.UtcTicks : -e.CreatedAt.UtcTicks)
                .Select(Snapshot).ToArray();
        }
    }

    internal JoinTransition Accept(Guid id, Guid seatId)
    {
        lock (_gate)
        {
            var now = clock.GetUtcNow();
            Sweep(now);
            var entry = _entries.FirstOrDefault(e => e.Id == id);
            if (entry is null) return JoinTransition.NotFound;
            if (entry.Status != JoinStatus.Pending) return JoinTransition.WrongState;
            entry.Status = JoinStatus.Accepted;
            entry.SeatId = seatId;
            entry.ChangedAt = now;
            return JoinTransition.Ok;
        }
    }

    internal JoinTransition Refuse(Guid id)
    {
        lock (_gate)
        {
            var now = clock.GetUtcNow();
            Sweep(now);
            var entry = _entries.FirstOrDefault(e => e.Id == id);
            if (entry is null) return JoinTransition.NotFound;
            if (entry.Status != JoinStatus.Pending) return JoinTransition.WrongState;
            entry.Status = JoinStatus.Refused;
            entry.ChangedAt = now;
            return JoinTransition.Ok;
        }
    }

    // ── Internals ───────────────────────────────────────────────────────────

    private Entry? Authenticate(Guid id, string? token)
    {
        var entry = _entries.FirstOrDefault(e => e.Id == id);
        var presented = Encoding.UTF8.GetBytes(token ?? string.Empty);
        // Always compare, even for an unknown id, so the answer does not depend on existence.
        var expected = entry?.Token is { } t ? Encoding.UTF8.GetBytes(Base64Url(t)) : new byte[43];
        var equal = CryptographicOperations.FixedTimeEquals(presented, expected);
        return equal && entry is not null ? entry : null;
    }

    /// <summary>Expires old requests and drops finished ones after <see cref="Retention"/>. Caller holds the lock.</summary>
    private void Sweep(DateTimeOffset now)
    {
        foreach (var e in _entries)
        {
            if (e.Status == JoinStatus.Pending && now - e.CreatedAt >= Lifetime)
            {
                e.Status = JoinStatus.Expired;
                e.ChangedAt = now;
            }
            else if (e.Status == JoinStatus.Accepted && !e.PinInProgress && now - e.ChangedAt >= Lifetime)
            {
                e.Status = JoinStatus.Expired;
                e.ChangedAt = now;
            }
        }
        _entries.RemoveAll(e => e.Status != JoinStatus.Pending && e.Status != JoinStatus.Accepted &&
            now - e.ChangedAt >= Retention);
        // Hard cap: never let finished entries pile up.
        while (_entries.Count > MaxEntries)
        {
            var oldest = _entries.Where(e => e.Status != JoinStatus.Pending && e.Status != JoinStatus.Accepted)
                .OrderBy(e => e.ChangedAt).FirstOrDefault();
            if (oldest is null) break;
            _entries.Remove(oldest);
        }
    }

    private static JoinRequestSnapshot Snapshot(Entry e) =>
        new(e.Id, e.Pseudo, e.Device, e.DeviceId, e.Address, e.CreatedAt, e.Status, e.Message, e.SeatId);

    internal static string Base64Url(byte[] bytes) =>
        Convert.ToBase64String(bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_');
}
