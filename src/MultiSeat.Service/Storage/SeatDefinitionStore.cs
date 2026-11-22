using System.Text.Json;
using MultiSeat.Shared.Models;

namespace MultiSeat.Service.Storage;

/// <summary>Durable definitions, never runtime process or session identifiers.</summary>
internal sealed record SeatDefinition(
    Guid Id, string AccountName, int Width, int Height, int Fps, int PortBase,
    string? LaunchApp, NvencQualityPreset NvencPreset, int? ScaleFactorOverride,
    bool AutoStart)
{
    internal static SeatDefinition From(SeatInfo seat) => new(
        seat.Id, seat.AccountName, seat.Width, seat.Height, seat.Fps, seat.PortBase,
        seat.LaunchApp, seat.NvencPreset, seat.ScaleFactorOverride, seat.AutoStart);

    internal SeatInfo ToSeat() => new()
    {
        Id = Id, AccountName = AccountName, Width = Width, Height = Height, Fps = Fps,
        PortBase = PortBase, LaunchApp = LaunchApp, NvencPreset = NvencPreset,
        ScaleFactorOverride = ScaleFactorOverride, AutoStart = AutoStart
    };
}

public sealed class SeatDefinitionStore
{
    private readonly string _path;
    private readonly ILogger<SeatDefinitionStore> _logger;
    private readonly object _sync = new();
    private readonly Dictionary<Guid, SeatDefinition> _definitions = new();

    public SeatDefinitionStore(ILogger<SeatDefinitionStore> logger, string? path = null)
    {
        _logger = logger;
        _path = path ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
            "MultiSeat", "seats.json");
        Load();
    }

    internal IReadOnlyList<SeatDefinition> GetAll()
    {
        lock (_sync) return [.. _definitions.Values];
    }

    internal void Upsert(SeatInfo seat)
    {
        lock (_sync)
        {
            var definition = SeatDefinition.From(seat);
            var hadPrevious = _definitions.TryGetValue(seat.Id, out var previous);
            if (hadPrevious && previous == definition) return;
            _definitions[seat.Id] = definition;
            try { Save(); }
            catch
            {
                if (hadPrevious) _definitions[seat.Id] = previous!;
                else _definitions.Remove(seat.Id);
                throw;
            }
        }
    }

    internal void Remove(Guid id)
    {
        lock (_sync)
        {
            if (!_definitions.Remove(id, out var previous)) return;
            try { Save(); }
            catch { _definitions[id] = previous; throw; }
        }
    }

    private void Load()
    {
        if (!File.Exists(_path)) return;
        try
        {
            if (OperatingSystem.IsWindows()) SecureFile.RestrictToSystemAndAdmins(_path);
            var entries = JsonSerializer.Deserialize<List<SeatDefinition>>(File.ReadAllText(_path))
                ?? throw new JsonException("Null seat definitions");
            foreach (var entry in entries)
            {
                if (entry.Id == Guid.Empty || string.IsNullOrWhiteSpace(entry.AccountName)
                    || entry.PortBase <= 0 || entry.Width < 640 || entry.Height < 480
                    || entry.Fps is < 1 or > 240 || !Enum.IsDefined(entry.NvencPreset)
                    || !_definitions.TryAdd(entry.Id, entry))
                    throw new JsonException("Invalid or duplicate seat definition");
            }
        }
        catch (JsonException ex)
        {
            _definitions.Clear();
            try { File.Move(_path, _path + ".bad", overwrite: true); }
            catch (Exception moveEx) { _logger.LogError(moveEx, "Could not quarantine {Path}", _path); }
            _logger.LogError(ex, "Corrupt seat definitions in {Path}; starting empty", _path);
        }
        catch (Exception ex)
        {
            _definitions.Clear();
            _logger.LogError(ex, "Could not load seat definitions from {Path}; starting empty", _path);
        }
    }

    private void Save()
    {
        var directory = Path.GetDirectoryName(_path)!;
        Directory.CreateDirectory(directory);
        var temp = Path.Combine(directory, $".{Path.GetFileName(_path)}.{Guid.NewGuid():N}.tmp");
        try
        {
            using (var stream = new FileStream(temp, FileMode.CreateNew, FileAccess.Write, FileShare.None))
            {
                JsonSerializer.Serialize(stream, _definitions.Values.ToList());
                stream.Flush(flushToDisk: true);
            }
            if (OperatingSystem.IsWindows()) SecureFile.RestrictToSystemAndAdmins(temp);
            File.Move(temp, _path, overwrite: true);
            if (OperatingSystem.IsWindows()) SecureFile.RestrictToSystemAndAdmins(_path);
        }
        finally { if (File.Exists(temp)) File.Delete(temp); }
    }
}
