using System.Diagnostics;
using System.Text.Json;
using Microsoft.Win32;
using MultiSeat.Service.Monitoring;
using MultiSeat.Service.Sessions;
using MultiSeat.Shared.Models;

namespace MultiSeat.Service.Gpu;

/// <summary>Advisory allocations only; Windows does not reserve VRAM for a session.</summary>
public sealed class GpuBudgetService
{
    private readonly Func<(string Name, long TotalMb)> _capacity;
    private readonly Func<int, int?> _usage;
    private readonly string _path;
    private readonly object _sync = new();
    private readonly SemaphoreSlim _mutations = new(1, 1);
    private readonly Dictionary<int, (DateTimeOffset At, int? Mb)> _samples = [];
    private int _hostReserveMb = 2048;

    public GpuBudgetService(GpuMonitor monitor) : this(() => Capacity(monitor), ReadSessionUsage) { }

    // Inject capacity and usage for deterministic tests, without Windows counters or a real GPU.
    public GpuBudgetService(Func<(string Name, long TotalMb)> capacity, Func<int, int?> usage,
        string? path = null)
    {
        _capacity = capacity;
        _usage = usage;
        _path = path ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
            "MultiSeat", "gpu-budget.json");
        if (!File.Exists(_path)) return;
        try
        {
            using var document = JsonDocument.Parse(File.ReadAllText(_path));
            var root = document.RootElement;
            if (root.ValueKind != JsonValueKind.Object || root.EnumerateObject().Count() != 1 ||
                !root.TryGetProperty("hostReserveMb", out var value) ||
                value.ValueKind != JsonValueKind.Number || !value.TryGetInt32(out var mb) ||
                mb is < 0 or > 262144)
                throw new InvalidDataException("Réserve VRAM invalide");
            _hostReserveMb = mb;
        }
        catch (Exception ex) { throw new InvalidDataException("Impossible de lire la réserve VRAM sans risque de l'écraser", ex); }
    }

    public async Task<IDisposable> AcquireMutationAsync(CancellationToken ct)
    {
        await _mutations.WaitAsync(ct);
        return new Release(_mutations);
    }

    private sealed class Release(SemaphoreSlim gate) : IDisposable
    {
        public void Dispose() => gate.Release();
    }

    public int HostReserveMb { get { lock (_sync) return _hostReserveMb; } }

    public void SetReserve(int mb)
    {
        if (mb is < 0 or > 262144) throw new ArgumentOutOfRangeException(nameof(mb));
        lock (_sync)
        {
            var dir = Path.GetDirectoryName(_path)!;
            Directory.CreateDirectory(dir);
            var staged = _path + "." + Guid.NewGuid().ToString("N") + ".tmp";
            try
            {
                using (var stream = new FileStream(staged, FileMode.CreateNew, FileAccess.Write, FileShare.None))
                {
                    JsonSerializer.Serialize(stream, new { hostReserveMb = mb });
                    stream.Flush(flushToDisk: true);
                }
                File.Move(staged, _path, overwrite: true);
                _hostReserveMb = mb;
            }
            finally { if (File.Exists(staged)) File.Delete(staged); }
        }
    }

    public object Snapshot(IReadOnlyCollection<SeatInfo> seats)
    {
        var (name, total) = ReadCapacity();
        var allocated = seats.Where(Allocated).Sum(s => (long)s.VramBudgetMb);
        var usable = Math.Max(0, total - HostReserveMb);
        return new
        {
            gpuName = name, totalMb = total, hostReserveMb = HostReserveMb,
            usableMb = usable, allocatedMb = allocated, freeMb = Math.Max(0, usable - allocated),
            seats = seats.Where(Allocated).Select(s => new
            {
                seatId = s.Id, accountName = s.AccountName, graphicsProfile = s.GraphicsProfile,
                vramBudgetMb = s.VramBudgetMb, vramUsedMb = Usage(s.SessionId)
            }).ToArray()
        };
    }

    private static bool Allocated(SeatInfo seat) => seat.Status is not (SeatStatus.Error or SeatStatus.Idle or SeatStatus.TearingDown);

    public void RequireFits(IReadOnlyCollection<SeatInfo> seats, int requestedMb, Guid? replacing = null)
    {
        var (_, total) = ReadCapacity();
        var allocated = seats.Where(s => Allocated(s) && s.Id != replacing).Sum(s => (long)s.VramBudgetMb);
        var free = Math.Max(0, total - HostReserveMb - allocated);
        if (!SeatGraphics.Fits(total, HostReserveMb,
                seats.Where(s => Allocated(s) && s.Id != replacing).Select(s => s.VramBudgetMb), requestedMb))
            throw new Ipc.AdminFault("insufficient_vram",
                $"La carte graphique n'a plus assez de mémoire libre : {free} Mo disponibles, {requestedMb} Mo demandés. Choisissez un profil plus léger ou augmentez la mémoire utilisable dans Paramètres.");
    }

    public int? Usage(int sessionId)
    {
        if (sessionId <= 0) return null;
        lock (_sync)
        {
            var now = DateTimeOffset.UtcNow;
            if (_samples.TryGetValue(sessionId, out var cached) && now - cached.At < TimeSpan.FromSeconds(5))
                return cached.Mb;
            int? value;
            try { value = _usage(sessionId); }
            catch { value = null; }
            _samples[sessionId] = (now, value);
            return value;
        }
    }

    private (string Name, long TotalMb) ReadCapacity()
    {
        try { return _capacity(); }
        catch { return (string.Empty, 0); }
    }

    private static (string Name, long TotalMb) Capacity(GpuMonitor monitor)
    {
        var info = monitor.Query();
        if (info is null) return (string.Empty, 0);
        if (info.VramTotalMb > 0 && (info.VramTotalMb > 4095 || info.Name.StartsWith("NVIDIA", StringComparison.OrdinalIgnoreCase)))
            return (info.Name, info.VramTotalMb);
        // WMI AdapterRAM is 32-bit and wraps on cards >= 4 GiB. Read the driver's 64-bit registry value.
        try
        {
            using var root = Registry.LocalMachine.OpenSubKey(
                @"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}");
            if (root is not null)
                foreach (var sub in root.GetSubKeyNames().Where(s => s.Length == 4 && s.All(char.IsDigit)).Order())
                {
                    using var key = root.OpenSubKey(sub);
                    var description = key?.GetValue("DriverDesc") as string;
                    if (!string.Equals(description, info.Name, StringComparison.OrdinalIgnoreCase)) continue;
                    var bytes = key?.GetValue("HardwareInformation.qwMemorySize") as byte[];
                    if (bytes is { Length: 8 })
                    {
                        var mb = BitConverter.ToUInt64(bytes) / (1024 * 1024);
                        if (mb > 0) return (info.Name, (long)mb);
                    }
                }
        }
        catch { /* unknown capacity must not be guessed from a truncated WMI value */ }
        return (info.Name, 0);
    }

    private static int? ReadSessionUsage(int sessionId)
    {
        if (!OperatingSystem.IsWindows()) return null;
        try
        {
            var category = new PerformanceCounterCategory("GPU Process Memory");
            var instances = category.GetInstanceNames();
            var pids = new HashSet<int>();
            foreach (var process in Process.GetProcesses())
            {
                using (process)
                {
                    try { if (process.SessionId == sessionId) pids.Add(process.Id); }
                    catch { /* process exited */ }
                }
            }
            long bytes = 0;
            foreach (var instance in instances)
            {
                if (!instance.StartsWith("pid_", StringComparison.OrdinalIgnoreCase)) continue;
                var end = instance.IndexOf('_', 4);
                if (end < 0 || !int.TryParse(instance.AsSpan(4, end - 4), out var pid) || !pids.Contains(pid)) continue;
                using var counter = new PerformanceCounter("GPU Process Memory", "Dedicated Usage", instance, readOnly: true);
                bytes = checked(bytes + counter.RawValue);
            }
            return (int)Math.Min(int.MaxValue, bytes / (1024 * 1024));
        }
        catch { return null; }
    }
}
