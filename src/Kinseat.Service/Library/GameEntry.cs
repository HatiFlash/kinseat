using System.Text.Json.Serialization;

namespace MultiSeat.Service.Library;

public sealed class GameEntry
{
    public required string Id { get; set; }
    public required string Name { get; set; }
    public required string Source { get; set; }
    public required string InstallPath { get; set; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? ExePath { get; set; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? LaunchUri { get; set; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public long? SizeBytes { get; set; }
    public List<string> AssignedTo { get; set; } = [];
    public string SaveIsolation { get; set; } = "auto";
    public bool StartFromHostSave { get; set; }
    public bool AntiCheat { get; set; }
    // Response-only fields; never saved to library.json.
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? SaveLocation { get; set; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? Store { get; set; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public Dictionary<string, string>? Access { get; set; }
}
