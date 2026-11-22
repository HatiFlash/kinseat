using System.Text.Json;

namespace MultiSeat.Service.Ipc;

public sealed class PreviewBroker : BackgroundService
{
    private sealed class Preview
    {
        internal long LastRequest;
        internal string? Jpeg;
        internal int Width, Height;
        internal DateTimeOffset? Captured;
        internal long ReceivedAt;
        internal string Caller = "?";
        internal DateTimeOffset Started;
    }
    private readonly Dictionary<Guid, Preview> _previews = [];
    private readonly object _gate = new();
    private readonly IAgentChannel _agent;
    private readonly TimeProvider _clock;
    private readonly ILogger<PreviewBroker> _logger;

    internal PreviewSettingsStore Settings { get; }

    public PreviewBroker(IAgentChannel agent, TimeProvider clock, ILogger<PreviewBroker> logger)
        : this(agent, clock, logger, new PreviewSettingsStore()) { }

    internal PreviewBroker(IAgentChannel agent, TimeProvider clock, ILogger<PreviewBroker> logger,
        PreviewSettingsStore settings)
    {
        _agent = agent;
        _clock = clock;
        _logger = logger;
        Settings = settings;
        _agent.Received += OnAgentMessage;
    }

    internal async Task<object> StartAsync(Guid seatId, CancellationToken ct, string caller = "?")
    {
        bool showIndicator;
        try { showIndicator = Settings.ShowIndicator; }
        catch (Exception ex) { throw new AdminFault("unavailable", $"Réglage d'aperçu illisible : {ex.GetType().Name}"); }
        lock (_gate)
        {
            if (_previews.Remove(seatId, out var previous)) LogEnd(seatId, previous, "replaced");
            _previews[seatId] = new Preview { LastRequest = _clock.GetTimestamp(),
                Started = _clock.GetUtcNow(), Caller = caller };
        }
        try { await _agent.SendAsync(seatId, new { type = "preview.start", maxWidth = 1280, fps = 4, showIndicator }, ct); }
        catch
        {
            lock (_gate) _previews.Remove(seatId);
            throw new AdminFault("unavailable", "Agent du siège indisponible");
        }
        _logger.LogInformation("Kinseat preview started seat {SeatId}, caller {Caller}, indicator {Indicator}", seatId, caller, showIndicator);
        return new { active = true };
    }

    internal async Task<object> StopAsync(Guid seatId, CancellationToken ct, string caller = "?")
    {
        lock (_gate)
        {
            if (_previews.Remove(seatId, out var previous)) LogEnd(seatId, previous, $"stopped by {caller}");
        }
        await StopAgentAsync(seatId, ct);
        return new { active = false };
    }

    private void LogEnd(Guid seatId, Preview previous, string reason) =>
        _logger.LogInformation("Kinseat preview ended seat {SeatId}, caller {Caller}, duration {DurationSeconds}s, reason {Reason}",
            seatId, previous.Caller, Math.Max(0, (_clock.GetUtcNow() - previous.Started).TotalSeconds), reason);

    internal object Frame(Guid seatId)
    {
        lock (_gate)
        {
            if (!_previews.TryGetValue(seatId, out var preview)) return new { active = false, live = false };
            preview.LastRequest = _clock.GetTimestamp();
            var age = preview.Captured is { } captured ?
                Math.Max(0, (_clock.GetUtcNow() - captured).TotalSeconds) : (double?)null;
            var live = preview.Captured is not null && _clock.GetElapsedTime(preview.ReceivedAt) <= TimeSpan.FromSeconds(3)
                && age <= 3;
            return new { active = true, jpegBase64 = preview.Jpeg, width = preview.Width,
                height = preview.Height, capturedAtUtc = preview.Captured, ageSeconds = age,
                live };
        }
    }

    private void OnAgentMessage(Guid seatId, JsonElement message)
    {
        if (!message.TryGetProperty("type", out var kind) || kind.ValueKind != JsonValueKind.String) return;
        lock (_gate)
        {
            if (!_previews.TryGetValue(seatId, out var preview)) return;
            if (kind.GetString() == "preview.failed")
            {
                _previews.Remove(seatId);
                LogEnd(seatId, preview, "agent failed");
                return;
            }
            if (kind.GetString() != "preview.frame"
                || !message.TryGetProperty("jpegBase64", out var jpeg) || jpeg.ValueKind != JsonValueKind.String
                || !message.TryGetProperty("width", out var width) || !width.TryGetInt32(out var w)
                || !message.TryGetProperty("height", out var height) || !height.TryGetInt32(out var h)
                || !message.TryGetProperty("capturedAtUtc", out var date)
                || !date.TryGetDateTimeOffset(out var timestamp)
                || w is < 1 or > 1280 || h is < 1 or > 1280 || timestamp > _clock.GetUtcNow()
                || jpeg.GetString() is not { } base64 || base64.Length > AgentFraming.MaxLineBytes) return;
            preview.Jpeg = base64;
            preview.Width = w;
            preview.Height = h;
            preview.Captured = timestamp;
            preview.ReceivedAt = _clock.GetTimestamp();
        }
    }

    internal async Task TickAsync(CancellationToken ct = default)
    {
        List<Guid> stopped = [];
        lock (_gate)
        {
            foreach (var (id, preview) in _previews)
                if (_clock.GetElapsedTime(preview.LastRequest) > TimeSpan.FromSeconds(10)) stopped.Add(id);
            foreach (var id in stopped)
            {
                var previous = _previews[id];
                _previews.Remove(id);
                LogEnd(id, previous, "observer timed out");
            }
        }
        foreach (var id in stopped) await StopAgentAsync(id, ct);
    }

    private async Task StopAgentAsync(Guid id, CancellationToken ct)
    {
        try { await _agent.SendAsync(id, new { type = "preview.stop" }, ct); }
        catch (Exception ex) { _logger.LogWarning(ex, "Could not stop preview for seat {SeatId}", id); }
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
            Guid[] ids;
            lock (_gate)
            {
                ids = _previews.Keys.ToArray();
                foreach (var (id, preview) in _previews) LogEnd(id, preview, "service stopping");
                _previews.Clear();
            }
            foreach (var id in ids) await StopAgentAsync(id, CancellationToken.None);
        }
    }
}
