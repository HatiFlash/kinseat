namespace MultiSeat.Service.Ipc;

/// <summary>Run independent cleanup actions without reporting a step as removed on failure.</summary>
internal sealed class SeatPurgeSteps
{
    internal sealed record Failure(string item, string reason);
    internal List<string> Removed { get; } = [];
    internal List<Failure> Failed { get; } = [];

    internal async Task Run(string item, Func<Task> action)
    {
        try { await action(); Removed.Add(item); }
        catch (Exception ex) { Failed.Add(new(item, ex.Message)); }
    }
}
