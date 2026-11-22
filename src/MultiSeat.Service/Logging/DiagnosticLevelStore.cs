using System.Text.Json;
using MultiSeat.Service.Storage;

namespace MultiSeat.Service.Logging;

public sealed class DiagnosticLevelStore(FileLoggerProvider provider, string? path = null)
{
    private readonly string _path = path ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData), "Kinseat", "logging.json");
    private readonly object _gate = new();
    internal string Level
    {
        get { lock (_gate) return provider.Detailed ? "Détaillé" : "Normal"; }
    }
    internal void Load()
    {
        lock (_gate)
        {
            if (!File.Exists(_path)) return;
            if (OperatingSystem.IsWindows()) SecureFile.RestrictToSystemAndAdmins(_path);
            var value = JsonSerializer.Deserialize<string>(File.ReadAllText(_path));
            if (value is "Normal" or "Détaillé") provider.SetLevel(value == "Détaillé");
        }
    }
    internal void Set(string value)
    {
        if (value is not ("Normal" or "Détaillé")) throw new ArgumentException("Niveau invalide", nameof(value));
        lock (_gate)
        {
            Directory.CreateDirectory(Path.GetDirectoryName(_path)!);
            var temp = _path + "." + Guid.NewGuid().ToString("N") + ".tmp";
            try
            {
                File.WriteAllText(temp, JsonSerializer.Serialize(value));
                if (OperatingSystem.IsWindows()) SecureFile.RestrictToSystemAndAdmins(temp);
                File.Move(temp, _path, true);
                provider.SetLevel(value == "Détaillé");
            }
            finally { if (File.Exists(temp)) File.Delete(temp); }
        }
    }
}
