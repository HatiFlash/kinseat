using System.Text.Json;
using MultiSeat.Shared.Models;

namespace MultiSeat.Service.Ipc;

internal enum NoticePhase { Pending, Sending, Displayed, Closing, Closed, Failed, Cancelled }

internal sealed class Notice
{
    public Guid Id { get; init; } = Guid.NewGuid();
    public required Guid SeatId { get; init; }
    public required string AccountName { get; init; }
    public required string Message { get; init; }
    public required int Minutes { get; init; }
    public required DateTimeOffset CreatedAtUtc { get; init; }
    public required DateTimeOffset DeadlineUtc { get; init; }
    public int RemainingSeconds { get; internal set; }
    public NoticePhase Phase { get; internal set; } = NoticePhase.Pending;
    public string? Detail { get; internal set; }
    public DateTimeOffset? ClosedAtUtc { get; internal set; }
}

/// <summary>Only the service's monotonic time determines deadlines; callbacks never execute under the lock.</summary>
public sealed class NoticeScheduler : BackgroundService
{
    private readonly object _gate = new();
    private readonly List<Notice> _notices = [];
    private readonly Dictionary<Guid, (long Deadline, long Wait, long Ack, long LastSend)> _timers = [];
    private readonly HashSet<Guid> _sending = [];
    private readonly TimeSpan _connectionTimeout;
    private readonly TimeProvider _clock;
    private readonly IAgentChannel _agent;
    private readonly Func<Guid, CancellationToken, Task> _teardown;
    private readonly ILogger<NoticeScheduler> _logger;

    public NoticeScheduler(TimeProvider clock, IAgentChannel agent,
        Func<Guid, CancellationToken, Task> teardown, ILogger<NoticeScheduler> logger,
        TimeSpan? connectionTimeout = null)
    {
        _connectionTimeout = connectionTimeout ?? TimeSpan.FromSeconds(60);
        if (_connectionTimeout <= TimeSpan.Zero) throw new ArgumentOutOfRangeException(nameof(connectionTimeout));
        _clock = clock;
        _agent = agent;
        _teardown = teardown;
        _logger = logger;
        _agent.Received += OnAgentMessage;
        _agent.Connected += OnAgentConnected;
        _agent.Disconnected += OnAgentDisconnected;
    }

    private void OnAgentDisconnected(Guid seatId)
    {
        lock (_gate)
        {
            foreach (var notice in _notices.Where(n => n.SeatId == seatId &&
                         n.Phase is NoticePhase.Sending or NoticePhase.Displayed))
            {
                // An acknowledgement belongs to the old process. Never tear down a seat
                // until the replacement has confirmed its own overlay is visible.
                notice.Phase = NoticePhase.Pending;
                if (_timers.TryGetValue(notice.Id, out var timer))
                    _timers[notice.Id] = (timer.Deadline,
                        _clock.GetTimestamp() + ToTicks(_connectionTimeout), 0, 0);
            }
        }
    }

    private void OnAgentConnected(Guid seatId)
    {
        // The hello has been validated by AgentHost. Do not wait for the next timer tick.
        _ = SendPendingAsync(seatId);
    }

    private async Task SendPendingAsync(Guid seatId)
    {
        try { await TickAsync(); }
        catch (Exception ex) { _logger.LogWarning(ex, "Could not dispatch pending notice for {SeatId}", seatId); }
    }

    internal IReadOnlyList<Notice> List()
    {
        lock (_gate)
        {
            RefreshRemaining();
            return _notices.Where(n => n.Phase is NoticePhase.Pending or NoticePhase.Sending or NoticePhase.Displayed or NoticePhase.Closing)
                .Concat(_notices.Where(n => n.Phase is NoticePhase.Closed or NoticePhase.Failed or NoticePhase.Cancelled)
                    .TakeLast(20)).ToArray();
        }
    }

    internal DateTimeOffset? LastHostClose(Guid seatId)
    {
        lock (_gate)
            return _notices.LastOrDefault(n => n.SeatId == seatId &&
                n.Phase is NoticePhase.Closing or NoticePhase.Closed)?.ClosedAtUtc;
    }

    internal Notice Schedule(SeatInfo seat, int minutes, string message)
    {
        if (seat.Status is not (SeatStatus.Ready or SeatStatus.Streaming))
            throw new AdminFault("conflict", "Le siège n'est pas prêt");
        lock (_gate)
        {
            if (_notices.Any(n => n.SeatId == seat.Id && n.Phase is NoticePhase.Pending or NoticePhase.Sending or NoticePhase.Displayed or NoticePhase.Closing))
                throw new AdminFault("conflict", "Un préavis est déjà actif");
            var now = _clock.GetTimestamp();
            var created = _clock.GetUtcNow();
            var notice = new Notice
            {
                SeatId = seat.Id, AccountName = seat.AccountName, Message = message, Minutes = minutes,
                CreatedAtUtc = created, DeadlineUtc = created.AddMinutes(minutes), RemainingSeconds = minutes * 60
            };
            _notices.Add(notice);
            _timers[notice.Id] = (now + ToTicks(TimeSpan.FromMinutes(minutes)),
                now + ToTicks(_connectionTimeout), 0, 0);
            return notice;
        }
    }

    internal Notice Cancel(Guid seatId)
    {
        lock (_gate)
        {
            var notice = _notices.LastOrDefault(n => n.SeatId == seatId);
            if (notice is null) throw new AdminFault("not_found", "Préavis introuvable");
            if (notice.Phase is NoticePhase.Closing or NoticePhase.Closed)
                throw new AdminFault("too_late", "La fermeture a déjà commencé");
            if (notice.Phase is NoticePhase.Failed or NoticePhase.Cancelled)
                throw new AdminFault("conflict", "Préavis déjà terminé");
            notice.Phase = NoticePhase.Cancelled;
            _timers.Remove(notice.Id);
            _ = HideAsync(notice);
            return notice;
        }
    }

    private long ToTicks(TimeSpan duration) => (long)(duration.TotalSeconds * _clock.TimestampFrequency);

    private void RefreshRemaining()
    {
        foreach (var notice in _notices)
            if (_timers.TryGetValue(notice.Id, out var timer))
                notice.RemainingSeconds = Math.Max(0,
                    (int)Math.Ceiling((timer.Deadline - _clock.GetTimestamp()) / (double)_clock.TimestampFrequency));
    }

    private void OnAgentMessage(Guid seatId, JsonElement message)
    {
        if (!message.TryGetProperty("type", out var kind) || kind.ValueKind != JsonValueKind.String ||
            !message.TryGetProperty("noticeId", out var id) || id.ValueKind != JsonValueKind.String ||
            !Guid.TryParse(id.GetString(), out var noticeId)) return;
        lock (_gate)
        {
            var notice = _notices.LastOrDefault(n => n.Id == noticeId && n.SeatId == seatId);
            if (notice?.Phase != NoticePhase.Sending || !_timers.TryGetValue(notice.Id, out var timer)
                || (timer.Ack == 0 && !_sending.Contains(notice.Id))) return;
            switch (kind.GetString())
            {
                case "notice.displayed": notice.Phase = NoticePhase.Displayed; break;
                case "notice.failed": Fail(notice, "Le préavis n'a pas pu être affiché"); break;
            }
        }
    }

    private void Fail(Notice notice, string detail)
    {
        notice.Phase = NoticePhase.Failed;
        notice.Detail = detail;
        _timers.Remove(notice.Id);
        _ = HideAsync(notice);
    }

    private async Task HideAsync(Notice notice)
    {
        try { await _agent.SendAsync(notice.SeatId, new { type = "notice.hide", noticeId = notice.Id }); }
        catch (Exception ex) { _logger.LogWarning(ex, "Could not hide notice {NoticeId}", notice.Id); }
    }

    internal async Task TickAsync(CancellationToken ct = default)
    {
        var shows = new List<Notice>();
        var closes = new List<Notice>();
        lock (_gate)
        {
            RefreshRemaining();
            var now = _clock.GetTimestamp();
            foreach (var notice in _notices)
            {
                if (!_timers.TryGetValue(notice.Id, out var timer)) continue;
                if (notice.Phase == NoticePhase.Pending && now >= timer.Wait)
                {
                    Fail(notice, "L'agent de session ne s'est pas connecté dans le délai imparti");
                    continue;
                }
                if (notice.Phase == NoticePhase.Sending && timer.Ack != 0 && now >= timer.Ack)
                {
                    Fail(notice, "Le préavis n'a pas pu être affiché");
                    continue;
                }
                if (now >= timer.Deadline && notice.Phase == NoticePhase.Displayed)
                {
                    notice.Phase = NoticePhase.Closing;
                    notice.ClosedAtUtc = _clock.GetUtcNow();
                    _timers.Remove(notice.Id);
                    closes.Add(notice);
                }
                else if ((notice.Phase == NoticePhase.Pending && _agent.IsConnected(notice.SeatId))
                    || (notice.Phase == NoticePhase.Displayed && now - timer.LastSend >= ToTicks(TimeSpan.FromSeconds(10))))
                {
                    if (_sending.Add(notice.Id)) shows.Add(notice);
                }
            }
        }
        foreach (var notice in shows)
        {
            try
            {
                lock (_gate)
                {
                    if (!_timers.ContainsKey(notice.Id) || notice.Phase is not (NoticePhase.Pending or NoticePhase.Displayed))
                        continue;
                    if (notice.Phase == NoticePhase.Pending) notice.Phase = NoticePhase.Sending;
                }
                await _agent.SendAsync(notice.SeatId, new
                { type = "notice.show", noticeId = notice.Id, message = notice.Message,
                  remainingSeconds = notice.RemainingSeconds }, ct);
                lock (_gate)
                {
                    if (_timers.TryGetValue(notice.Id, out var timer))
                    {
                        var sent = _clock.GetTimestamp();
                        _timers[notice.Id] = (timer.Deadline, timer.Wait,
                            notice.Phase == NoticePhase.Sending && timer.Ack == 0
                                ? sent + ToTicks(TimeSpan.FromSeconds(15)) : timer.Ack, sent);
                    }
                }
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested) { throw; }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "Could not send notice {NoticeId}", notice.Id);
                lock (_gate)
                    if (notice.Phase == NoticePhase.Sending && _timers.TryGetValue(notice.Id, out var timer)
                        && timer.Ack == 0)
                        notice.Phase = NoticePhase.Pending;
            }
            finally { lock (_gate) _sending.Remove(notice.Id); }
        }
        foreach (var notice in closes)
        {
            try
            {
                await _teardown(notice.SeatId, ct);
                lock (_gate) notice.Phase = NoticePhase.Closed;
            }
            catch (Exception ex)
            {
                _logger.LogError(ex, "Notice {NoticeId}: teardown failed", notice.Id);
                lock (_gate) { notice.Phase = NoticePhase.Failed; notice.Detail = "La fermeture du siège a échoué"; }
            }
        }
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        try
        {
            while (!stoppingToken.IsCancellationRequested)
            {
                await TickAsync(stoppingToken);
                await Task.Delay(TimeSpan.FromSeconds(1), _clock, stoppingToken);
            }
        }
        catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested) { }
        finally
        {
            lock (_gate)
                foreach (var notice in _notices.Where(n => n.Phase is NoticePhase.Pending or NoticePhase.Sending or NoticePhase.Displayed))
                {
                    notice.Phase = NoticePhase.Cancelled;
                    _logger.LogInformation("Préavis {NoticeId} annulé au redémarrage/arrêt du service", notice.Id);
                }
        }
    }
}

internal sealed class AdminFault(string code, string message) : Exception(message)
{
    internal string Code { get; } = code;
}
