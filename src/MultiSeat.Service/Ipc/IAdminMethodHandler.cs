using System.Text.Json;

namespace MultiSeat.Service.Ipc;

/// <summary>Additional admin-only methods; invoked after the pipe's identity authorization.</summary>
public interface IAdminMethodHandler
{
    IReadOnlyCollection<string> Methods { get; }
    Task<object> HandleAsync(string method, JsonElement parameters, CancellationToken ct);
}
