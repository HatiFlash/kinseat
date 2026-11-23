using System.Collections.Concurrent;
using System.Diagnostics;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Runtime.CompilerServices;
using System.Text.Json;
using Microsoft.Extensions.Options;
using MultiSeat.Service.Configuration;
using MultiSeat.Service.Sessions;
using MultiSeat.Shared.Models;

namespace MultiSeat.Service.Ipc;

public interface IAgentChannel
{
    event Action<Guid, JsonElement>? Received;
    event Action<Guid>? Connected;
    event Action<Guid>? Disconnected;
    bool IsConnected(Guid seatId);
    Task SendAsync(Guid seatId, object message, CancellationToken ct = default);
}

internal static class AgentFraming
{
    internal const int MaxLineBytes = 4 * 1024 * 1024;
    // The wire format is newline-delimited JSON, not length-prefixed. Keep read-ahead
    // between calls so a single read containing several frames loses none of them.
    private sealed class ReadState
    {
        internal readonly byte[] Bytes = new byte[16 * 1024];
        internal int Offset;
        internal int Count;
    }
    private static readonly ConditionalWeakTable<Stream, ReadState> States = new();

    internal static async Task<JsonElement?> ReadAsync(Stream stream, CancellationToken ct)
    {
        var state = States.GetValue(stream, static _ => new ReadState());
        using var buffer = new MemoryStream();
        while (true)
        {
            if (state.Offset == state.Count)
            {
                state.Count = await stream.ReadAsync(state.Bytes, ct);
                state.Offset = 0;
                if (state.Count == 0)
                {
                    if (buffer.Length != 0) throw new EndOfStreamException("Agent line truncated");
                    return null;
                }
            }
            var remaining = state.Bytes.AsSpan(state.Offset, state.Count - state.Offset);
            var newline = remaining.IndexOf((byte)'\n');
            var length = newline < 0 ? remaining.Length : newline;
            if (buffer.Length + length >= MaxLineBytes)
                throw new InvalidDataException("Agent line exceeds 4 MiB");
            buffer.Write(state.Bytes, state.Offset, length);
            state.Offset += length + (newline < 0 ? 0 : 1);
            if (newline < 0) continue;
            try
            {
                using var doc = JsonDocument.Parse(new ReadOnlyMemory<byte>(buffer.GetBuffer(), 0, (int)buffer.Length));
                return doc.RootElement.Clone();
            }
            catch (JsonException ex) { throw new InvalidDataException("Invalid agent JSON frame", ex); }
        }
    }

    internal static async Task WriteAsync(Stream stream, object message, CancellationToken ct)
    {
        var bytes = JsonSerializer.SerializeToUtf8Bytes(message);
        if (bytes.Length + 1 > MaxLineBytes) throw new InvalidDataException("Agent line exceeds 4 MiB");
        await stream.WriteAsync(bytes, ct);
        await stream.WriteAsync(new byte[] { (byte)'\n' }, ct);
        await stream.FlushAsync(ct);
    }
}

/// <summary>Owns the per-seat process and authenticated pipe; no seat data is written to disk.</summary>
internal sealed class AgentHost(
    SeatManager seats, SessionLauncher launcher, IOptions<MultiSeatOptions> options,
    ILogger<AgentHost> logger) : BackgroundService, IAgentChannel
{
    private sealed class Connection
    {
        internal required Process Process;
        internal NamedPipeServerStream? Pipe;
        internal bool Ready;
        internal readonly SemaphoreSlim SendLock = new(1, 1);
    }

    private readonly ConcurrentDictionary<Guid, Connection> _connections = new();
    private sealed class RetryState
    {
        internal readonly Queue<DateTimeOffset> Attempts = new();
        internal DateTimeOffset Next;
        internal int Failures;
    }
    private readonly Dictionary<Guid, RetryState> _retries = new();
    public event Action<Guid, JsonElement>? Received;
    public event Action<Guid>? Connected;
    public event Action<Guid>? Disconnected;
    public bool IsConnected(Guid seatId) => _connections.TryGetValue(seatId, out var connection)
        && connection.Ready && connection.Pipe?.IsConnected == true;

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetNamedPipeClientProcessId(SafeHandle pipe, out uint pid);

    private static PipeSecurity SecurityFor(string accountName)
    {
        var sid = (SecurityIdentifier)new NTAccount(accountName).Translate(typeof(SecurityIdentifier));
        var security = new PipeSecurity();
        security.SetAccessRuleProtection(true, false);
        security.AddAccessRule(new PipeAccessRule(new SecurityIdentifier(WellKnownSidType.NetworkSid, null),
            PipeAccessRights.FullControl, AccessControlType.Deny));
        foreach (var allowed in new[] { new SecurityIdentifier(WellKnownSidType.LocalSystemSid, null), sid })
            security.AddAccessRule(new PipeAccessRule(allowed, PipeAccessRights.FullControl, AccessControlType.Allow));
        return security;
    }

    protected override async Task ExecuteAsync(CancellationToken ct)
    {
        if (!OperatingSystem.IsWindows()) return;
        try
        {
            while (!ct.IsCancellationRequested)
            {
                foreach (var seat in seats.GetAllSeats())
                {
                    if (seat.Status is SeatStatus.Ready or SeatStatus.Streaming && seat.SessionId >= 0
                        && !_connections.ContainsKey(seat.Id) && CanStart(seat.Id))
                        Start(seat, ct);
                }
                foreach (var (id, connection) in _connections)
                {
                    var seat = seats.GetSeat(id);
                    if (seat is null || seat.Status is SeatStatus.TearingDown or SeatStatus.Error)
                    {
                        Stop(id);
                        _retries.Remove(id);
                    }
                    else if (_connections.TryGetValue(id, out var current) &&
                             ReferenceEquals(current, connection) && ProcessExited(connection.Process))
                    {
                        Stop(id, connection);
                    }
                }
                await Task.Delay(TimeSpan.FromSeconds(1), ct);
            }
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested) { }
        finally { foreach (var id in _connections.Keys) Stop(id); }
    }

    private bool ProcessExited(Process process)
    {
        try
        {
            if (!process.HasExited) return false;
            logger.LogWarning("Seat agent PID {Pid} exited with code {ExitCode}", process.Id, process.ExitCode);
            return true;
        }
        catch (InvalidOperationException) { return false; } // ReadLoop disposed it first.
    }

    private bool CanStart(Guid seatId)
    {
        if (!_retries.TryGetValue(seatId, out var retry))
            _retries[seatId] = retry = new RetryState();
        var now = DateTimeOffset.UtcNow;
        while (retry.Attempts.TryPeek(out var oldest) && now - oldest >= TimeSpan.FromMinutes(1))
            retry.Attempts.Dequeue();
        if (now < retry.Next || retry.Attempts.Count >= 5) return false;
        retry.Attempts.Enqueue(now);
        retry.Failures = Math.Min(retry.Failures + 1, 5);
        var delay = Math.Min(30, 1 << retry.Failures);
        retry.Next = now.AddSeconds(delay);
        if (retry.Attempts.Count == 5 && retry.Next < retry.Attempts.Peek().AddMinutes(1))
            retry.Next = retry.Attempts.Peek().AddMinutes(1);
        return true;
    }

    private void Start(SeatInfo seat, CancellationToken ct)
    {
        var exe = options.Value.KinseatAgentExePath ?? Path.Combine(AppContext.BaseDirectory, "kinseat-agent.exe");
        if (!Path.IsPathFullyQualified(exe) || !File.Exists(exe)) return;
        var pipeName = $"Kinseat.Agent.v1.{seat.Id:D}";
        NamedPipeServerStream? pipe = null;
        Process? process = null;
        try
        {
            pipe = NamedPipeServerStreamAcl.Create(pipeName, PipeDirection.InOut, 1,
                PipeTransmissionMode.Byte, PipeOptions.Asynchronous, 0, 0, SecurityFor(seat.AccountName));
            process = launcher.LaunchPersistentHelperInSeatSession(seat.SessionId, seat.AccountName,
                $"\"{exe}\" --seat {seat.Id:D} --pipe \"\\\\.\\pipe\\{pipeName}\"");
            var connection = new Connection { Process = process, Pipe = pipe };
            if (!_connections.TryAdd(seat.Id, connection))
            {
                process.Kill();
                process.Dispose();
                pipe.Dispose();
                return;
            }
            _ = ReadLoopAsync(seat.Id, connection, ct);
        }
        catch (Exception ex)
        {
            logger.LogWarning(ex, "Could not launch agent for seat {SeatId}", seat.Id);
            pipe?.Dispose();
            process?.Dispose();
        }
    }

    private async Task ReadLoopAsync(Guid seatId, Connection connection, CancellationToken ct)
    {
        try
        {
            var pipe = connection.Pipe!;
            await pipe.WaitForConnectionAsync(ct);
            if (!GetNamedPipeClientProcessId(pipe.SafePipeHandle, out var pid) || pid != connection.Process.Id)
                throw new UnauthorizedAccessException("Agent PID mismatch");
            var hello = await AgentFraming.ReadAsync(pipe, ct);
            if (hello is null || !hello.Value.TryGetProperty("type", out var type) || type.GetString() != "hello"
                || !hello.Value.TryGetProperty("pid", out var claimed) || claimed.GetInt32() != pid
                || !hello.Value.TryGetProperty("sessionId", out var session)
                || session.GetInt32() != seats.GetSeat(seatId)?.SessionId)
                throw new UnauthorizedAccessException("Agent hello mismatch");
            connection.Ready = true;
            logger.LogInformation("Seat agent {SeatId} connected (PID {Pid})", seatId, pid);
            Connected?.Invoke(seatId);
            JsonElement? message;
            while ((message = await AgentFraming.ReadAsync(pipe, ct)) is { } value)
                Received?.Invoke(seatId, value);
            logger.LogWarning("Seat agent {SeatId} closed pipe (EOF)", seatId);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested) { }
        catch (Exception ex) { logger.LogWarning(ex, "Agent pipe disconnected for {SeatId}", seatId); }
        finally { Stop(seatId, connection); }
    }

    public async Task SendAsync(Guid seatId, object message, CancellationToken ct = default)
    {
        if (!_connections.TryGetValue(seatId, out var connection) || !connection.Ready || connection.Pipe?.IsConnected != true)
            throw new IOException("Seat agent unavailable");
        await connection.SendLock.WaitAsync(ct);
        try
        {
            if (!connection.Ready || connection.Pipe?.IsConnected != true)
                throw new IOException("Seat agent unavailable");
            await AgentFraming.WriteAsync(connection.Pipe, message, ct);
        }
        finally { connection.SendLock.Release(); }
    }

    private void Stop(Guid seatId, Connection? expected = null)
    {
        if (!_connections.TryGetValue(seatId, out var connection) ||
            (expected is not null && !ReferenceEquals(connection, expected)) ||
            !_connections.TryRemove(new KeyValuePair<Guid, Connection>(seatId, connection))) return;
        var wasReady = connection.Ready;
        connection.Ready = false;
        connection.Pipe?.Dispose();
        if (wasReady)
        {
            logger.LogWarning("Seat agent {SeatId} disconnected", seatId);
            try { Disconnected?.Invoke(seatId); }
            catch (Exception ex) { logger.LogWarning(ex, "Agent disconnect callback failed for {SeatId}", seatId); }
        }
        try { if (!connection.Process.HasExited) connection.Process.Kill(); }
        catch (InvalidOperationException) { }
        catch (System.ComponentModel.Win32Exception ex) { logger.LogWarning(ex, "Could not stop seat agent {SeatId}", seatId); }
        connection.Process.Dispose();
    }
}
