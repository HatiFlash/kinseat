using System.Text;
using System.Text.Json;

namespace MultiSeat.Service.Ipc;

/// <summary>Single service-owned preference; absent file means opt-in indicator on.</summary>
internal sealed class PreviewSettingsStore
{
    private readonly object _gate = new();
    private readonly string _path;

    internal PreviewSettingsStore(string? path = null) => _path = path ?? Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
        "MultiSeat", "kinseat-preview-settings.json");

    internal bool ShowIndicator
    {
        get
        {
            lock (_gate)
            {
                if (!File.Exists(_path)) return true;
                using var document = JsonDocument.Parse(File.ReadAllText(_path));
                var root = document.RootElement;
                if (root.ValueKind != JsonValueKind.Object || root.EnumerateObject().Count() != 1 ||
                    !root.TryGetProperty("preview", out var preview) || preview.ValueKind != JsonValueKind.Object ||
                    preview.EnumerateObject().Count() != 1 ||
                    !preview.TryGetProperty("showIndicator", out var value) ||
                    value.ValueKind is not (JsonValueKind.True or JsonValueKind.False))
                    throw new InvalidDataException("Réglage d'aperçu invalide");
                return value.GetBoolean();
            }
        }
    }

    internal void SetShowIndicator(bool enabled)
    {
        lock (_gate)
        {
            // Never overwrite a damaged preference silently.
            _ = ShowIndicator;
            var directory = Path.GetDirectoryName(_path)!;
            Directory.CreateDirectory(directory);
            var staged = _path + "." + Guid.NewGuid().ToString("N") + ".tmp";
            try
            {
                using (var stream = new FileStream(staged, FileMode.CreateNew, FileAccess.Write, FileShare.None))
                {
                    var bytes = Encoding.UTF8.GetBytes(JsonSerializer.Serialize(new
                    {
                        preview = new { showIndicator = enabled }
                    }));
                    stream.Write(bytes);
                    stream.Flush(flushToDisk: true);
                }
                File.Move(staged, _path, overwrite: true);
            }
            finally { if (File.Exists(staged)) File.Delete(staged); }
        }
    }
}
