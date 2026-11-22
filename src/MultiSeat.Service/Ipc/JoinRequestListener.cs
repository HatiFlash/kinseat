using System.Net;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using MultiSeat.Service.Sessions;
using MultiSeat.Shared;
using MultiSeat.Shared.Models;

namespace MultiSeat.Service.Ipc;

/// <summary>
/// LAN listener for join requests (docs/join-requests.md). Its own Kestrel instance on
/// <see cref="Constants.JoinRequestPort"/>, separate from the loopback admin API, serving exactly
/// five routes and no API key: a request grants nothing until the Host app accepts it.
/// </summary>
public sealed class JoinRequestListener(
    JoinRequestStore store, SeatManager seats, NoticeScheduler notices, TimeProvider clock,
    ILogger<JoinRequestListener> logger) : BackgroundService
{
    internal const int MaxBodyBytes = 4096;
    private const string ApiPrefix = "/kinseat/v1/";
    private static readonly TimeSpan PairTimeout = TimeSpan.FromSeconds(45);
    private static readonly JsonSerializerOptions JsonWithNulls = new(JsonSerializerDefaults.Web);
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web)
    {
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull
    };

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        WebApplication? app = null;
        try
        {
            var builder = WebApplication.CreateSlimBuilder(new WebApplicationOptions
            {
                ContentRootPath = AppContext.BaseDirectory,
            });
            builder.Logging.ClearProviders();
            builder.WebHost.ConfigureKestrel(kestrel =>
            {
                kestrel.AddServerHeader = false;
                kestrel.Limits.MaxRequestBodySize = MaxBodyBytes;
                kestrel.Limits.MaxRequestHeadersTotalSize = 8192;
                kestrel.Limits.MaxConcurrentConnections = 32;
                kestrel.Limits.RequestHeadersTimeout = TimeSpan.FromSeconds(5);
                kestrel.ListenAnyIP(Constants.JoinRequestPort);
            });
            app = builder.Build();
            app.Run(HandleAsync);
            await app.StartAsync(stoppingToken);
            logger.LogInformation("Kinseat join-request listener on TCP {Port}", Constants.JoinRequestPort);
            await Task.Delay(Timeout.Infinite, stoppingToken);
        }
        catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested) { }
        catch (Exception ex)
        {
            // Never take the service down for a listener that cannot bind (port in use, ...).
            logger.LogError(ex, "Kinseat join-request listener failed on TCP {Port}", Constants.JoinRequestPort);
        }
        finally
        {
            if (app is not null)
            {
                try { await app.StopAsync(TimeSpan.FromSeconds(3)); await app.DisposeAsync(); }
                catch (Exception) { /* shutting down */ }
            }
        }
    }

    private async Task HandleAsync(HttpContext ctx)
    {
        try
        {
            await RouteAsync(ctx);
        }
        catch (OperationCanceledException) { }
        catch (BadHttpRequestException) { ctx.Response.StatusCode = StatusCodes.Status413PayloadTooLarge; }
        catch (Exception ex)
        {
            logger.LogWarning(ex, "Kinseat join-request handler failed");
            if (!ctx.Response.HasStarted) await WriteAsync(ctx, 500, new { error = "internal_error" });
        }
    }

    private async Task RouteAsync(HttpContext ctx)
    {
        var path = ctx.Request.Path.Value ?? "";
        var method = ctx.Request.Method;
        if (!path.StartsWith(ApiPrefix, StringComparison.Ordinal)) { await WriteAsync(ctx, 404, null); return; }
        var rest = path[ApiPrefix.Length..].TrimEnd('/');
        var parts = rest.Split('/');

        if (parts is ["hello"])
        {
            if (!HttpMethods.IsGet(method)) { await WriteAsync(ctx, 405, null); return; }
            var v = typeof(JoinRequestListener).Assembly.GetName().Version;
            var version = v is null ? "0.0.0" : $"{v.Major}.{v.Minor}.{Math.Max(v.Build, 0)}";
            await WriteAsync(ctx, 200, new { kinseat = true, host = Environment.MachineName, version });
            return;
        }
        if (parts is ["join"])
        {
            if (!HttpMethods.IsPost(method)) { await WriteAsync(ctx, 405, null); return; }
            await JoinAsync(ctx);
            return;
        }
        if (parts is ["join", var idText] && Guid.TryParse(idText, out var id))
        {
            if (!HttpMethods.IsGet(method)) { await WriteAsync(ctx, 405, null); return; }
            var snapshot = store.Get(id, Token(ctx));
            if (snapshot is null) { await WriteAsync(ctx, 404, null); return; }
            await WriteAsync(ctx, 200, StatusBody(ctx, snapshot));
            return;
        }
        if (parts is ["join", var sessionIdText, "session"] && Guid.TryParse(sessionIdText, out var sessionId))
        {
            if (!HttpMethods.IsGet(method)) { await WriteAsync(ctx, 405, null); return; }
            await SessionAsync(ctx, sessionId, store, notices, clock.GetUtcNow());
            return;
        }
        if (parts is ["join", var idText2, "pin"] && Guid.TryParse(idText2, out var pinId))
        {
            if (!HttpMethods.IsPost(method)) { await WriteAsync(ctx, 405, null); return; }
            await PinAsync(ctx, pinId);
            return;
        }
        await WriteAsync(ctx, 404, null);
    }

    internal static async Task SessionAsync(HttpContext ctx, Guid id, JoinRequestStore store,
        NoticeScheduler notices, DateTimeOffset now)
    {
        var snapshot = store.Get(id, Token(ctx));
        if (snapshot is null) { await WriteAsync(ctx, 404, null); return; }
        var closedAt = snapshot.Status is JoinStatus.Accepted or JoinStatus.Paired && snapshot.SeatId is { } seatId
            ? notices.LastHostClose(seatId) : null;
        var recent = RecentHostClose(closedAt, now);
        await WriteAsync(ctx, 200, new { closedByHost = recent is not null, closedAtUtc = recent }, includeNull: true);
    }

    // A future timestamp cannot be evidence of a recent closure (e.g. after a clock adjustment).
    internal static DateTimeOffset? RecentHostClose(DateTimeOffset? closedAt, DateTimeOffset now) =>
        closedAt is { } at && now >= at && now - at <= TimeSpan.FromSeconds(120) ? at : null;

    private async Task JoinAsync(HttpContext ctx)
    {
        var doc = await ReadJsonAsync(ctx);
        if (doc is null) { await WriteAsync(ctx, 400, new { error = "invalid_request" }); return; }
        using (doc)
        {
            var root = doc.RootElement;
            var pseudo = Str(root, "pseudo");
            var device = Str(root, "device");
            var deviceId = Str(root, "deviceId");
            if (!JoinRequestStore.IsValidPseudo(pseudo) || !JoinRequestStore.IsValidDevice(device) ||
                !JoinRequestStore.IsValidDeviceId(deviceId))
            {
                await WriteAsync(ctx, 400, new { error = "invalid_request" });
                return;
            }
            var result = store.Submit(pseudo!, device!, deviceId!, RemoteAddress(ctx));
            if (result.Outcome != JoinSubmitOutcome.Created)
            {
                ctx.Response.Headers["Retry-After"] = "60";
                await WriteAsync(ctx, 429, new { error = "too_many_requests" });
                return;
            }
            logger.LogInformation("Kinseat join request {Id} from {Address}", result.RequestId, RemoteAddress(ctx));
            await WriteAsync(ctx, 202, new { requestId = result.RequestId, token = result.Token });
        }
    }

    private async Task PinAsync(HttpContext ctx, Guid id)
    {
        var token = Token(ctx);
        var current = store.Get(id, token);
        if (current is null) { await WriteAsync(ctx, 404, null); return; }
        if (current.Status != JoinStatus.Accepted)
        {
            await WriteAsync(ctx, 409, new { error = "not_accepted", status = StatusName(current.Status) });
            return;
        }
        var doc = await ReadJsonAsync(ctx);
        var pin = doc is null ? null : Str(doc.RootElement, "pin");
        doc?.Dispose();
        if (pin is not { Length: 4 } || !pin.All(c => c is >= '0' and <= '9'))
        {
            await WriteAsync(ctx, 400, new { error = "invalid_pin" });
            return;
        }
        var claimed = store.BeginPin(id, token);
        if (claimed is null || claimed.SeatId is not { } seatId)
        {
            await WriteAsync(ctx, 409, new { error = "not_accepted" });
            return;
        }
        string? failure = null;
        try
        {
            using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ctx.RequestAborted);
            timeout.CancelAfter(PairTimeout);
            await seats.SubmitPairingPinAsync(seatId, pin, claimed.Device, timeout.Token);
        }
        catch (SeatNotFoundException) { failure = "Place introuvable"; }
        catch (ResourceConflictException) { failure = "La place n'est pas prête"; }
        catch (ArgumentException) { failure = "Code ou nom d'appareil invalide"; }
        catch (InvalidOperationException ex) { failure = string.IsNullOrWhiteSpace(ex.Message) ? "L'appairage a échoué" : ex.Message; }
        catch (OperationCanceledException) { failure = "L'appairage a pris trop de temps"; }
        catch (Exception ex)
        {
            logger.LogWarning(ex, "Kinseat join request {Id}: pairing failed", id);
            failure = "L'appairage a échoué";
        }
        if (failure is null)
        {
            store.MarkPaired(id);
            logger.LogInformation("Kinseat join request {Id}: device paired", id);
            await WriteAsync(ctx, 200, new { status = "paired" });
        }
        else
        {
            store.MarkFailed(id, failure);
            await WriteAsync(ctx, 200, new { status = "failed", message = failure });
        }
    }

    private object StatusBody(HttpContext ctx, JoinRequestSnapshot s)
    {
        object? place = null;
        if (s.Status is JoinStatus.Accepted or JoinStatus.Paired && s.SeatId is { } seatId &&
            seats.GetSeat(seatId) is { } seat)
        {
            var local = ctx.Connection.LocalIpAddress;
            if (local is not null && local.IsIPv4MappedToIPv6) local = local.MapToIPv4();
            place = new
            {
                name = string.IsNullOrWhiteSpace(seat.DisplayName) ? seat.AccountName : seat.DisplayName,
                address = local?.ToString() ?? "",
                port = seat.PortBase + Constants.OffsetGfeHttp
            };
        }
        return new { status = StatusName(s.Status), message = s.Message, place };
    }

    internal static string StatusName(JoinStatus status) => status.ToString().ToLowerInvariant();

    private static string? Token(HttpContext ctx) =>
        ctx.Request.Headers.TryGetValue("X-Kinseat-Token", out var v) && v.Count == 1 ? v[0] : null;

    private static string RemoteAddress(HttpContext ctx)
    {
        var ip = ctx.Connection.RemoteIpAddress ?? IPAddress.None;
        if (ip.IsIPv4MappedToIPv6) ip = ip.MapToIPv4();
        return ip.ToString();
    }

    private static string? Str(JsonElement root, string name) =>
        root.ValueKind == JsonValueKind.Object && root.TryGetProperty(name, out var p) &&
        p.ValueKind == JsonValueKind.String ? p.GetString() : null;

    /// <summary>Reads at most <see cref="MaxBodyBytes"/> bytes; null when absent, too large or not JSON.</summary>
    private static async Task<JsonDocument?> ReadJsonAsync(HttpContext ctx)
    {
        if (ctx.Request.ContentLength is > MaxBodyBytes) return null;
        using var buffer = new MemoryStream();
        var chunk = new byte[1024];
        int read;
        while ((read = await ctx.Request.Body.ReadAsync(chunk, ctx.RequestAborted)) > 0)
        {
            if (buffer.Length + read > MaxBodyBytes) return null;
            buffer.Write(chunk, 0, read);
        }
        if (buffer.Length == 0) return null;
        try { return JsonDocument.Parse(buffer.ToArray()); }
        catch (JsonException) { return null; }
    }

    private static async Task WriteAsync(HttpContext ctx, int status, object? body, bool includeNull = false)
    {
        ctx.Response.StatusCode = status;
        ctx.Response.Headers.CacheControl = "no-store";
        if (body is null) return;
        ctx.Response.ContentType = "application/json; charset=utf-8";
        await ctx.Response.Body.WriteAsync(JsonSerializer.SerializeToUtf8Bytes(body,
            includeNull ? JsonWithNulls : Json));
    }
}
