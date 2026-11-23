using System.Diagnostics.Eventing.Reader;
using System.IO.Compression;
using System.Management;
using System.Text;
using System.Text.Json;
using MultiSeat.Service.Logging;
using MultiSeat.Service.Storage;

namespace MultiSeat.Service.Ipc;

/// <summary>Creates a bounded, sanitised support archive. No credentials or raw binary files enter it.</summary>
internal static class DiagnosticExporter
{
    private const long MaxInputBytes = 45 * 1024 * 1024;
    internal static string Export(object state, IEnumerable<string> accounts, string? programData = null,
        string? usersRoot = null, string? outputDirectory = null, string? hostAccount = null)
    {
        programData ??= Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData);
        usersRoot ??= Path.Combine(Environment.GetEnvironmentVariable("SystemDrive") ?? "C:", "Users");
        outputDirectory ??= Path.Combine(programData, "Kinseat", "diagnostics");
        Directory.CreateDirectory(outputDirectory);
        var name = $"kinseat-diagnostic-{DateTime.UtcNow:yyyyMMdd-HHmmss}-{Guid.NewGuid():N}.zip";
        var path = Path.Combine(outputDirectory, name);
        try
        {
            // Restrict before writing content: no transient world-readable archive.
            using (var file = new FileStream(path, FileMode.CreateNew, FileAccess.ReadWrite, FileShare.None))
            {
                if (OperatingSystem.IsWindows()) SecureFile.RestrictToSystemAndAdmins(path);
                using var zip = new ZipArchive(file, ZipArchiveMode.Create, leaveOpen: true);
                long total = 0;
                void AddText(string entry, string content)
                {
                    var bytes = Encoding.UTF8.GetBytes(DiagnosticRedactor.Clean(content));
                    if (bytes.LongLength + total > MaxInputBytes) return;
                    total += bytes.LongLength;
                    using var writer = new StreamWriter(zip.CreateEntry(entry, CompressionLevel.Fastest).Open(), Encoding.UTF8);
                    writer.Write(Encoding.UTF8.GetString(bytes));
                }
                static bool HasReparsePoint(string filePath, string? stopAt)
                {
                    try
                    {
                        var current = Path.GetFullPath(filePath);
                        var limit = string.IsNullOrEmpty(stopAt) ? null : Path.GetFullPath(stopAt);
                        while (!string.IsNullOrEmpty(current))
                        {
                            if ((File.Exists(current) || Directory.Exists(current)) &&
                                (File.GetAttributes(current) & FileAttributes.ReparsePoint) != 0)
                                return true;
                            if (limit is not null && string.Equals(current, limit, StringComparison.OrdinalIgnoreCase))
                                break;
                            var parent = Path.GetDirectoryName(current);
                            if (string.IsNullOrEmpty(parent) || string.Equals(parent, current, StringComparison.OrdinalIgnoreCase))
                                break;
                            current = parent;
                        }
                        return false;
                    }
                    catch (Exception) { return true; } // fail closed on inaccessible metadata
                }
                void AddFile(string source, string entry, string? stopAt = null)
                {
                    try
                    {
                        if (!File.Exists(source) || HasReparsePoint(source, stopAt) || new FileInfo(source).Length > 5 * 1024 * 1024) return;
                        AddText(entry, File.ReadAllText(source));
                    }
                    catch (Exception) { /* Unavailable or locked log: continue with the rest. */ }
                }
                AddText("LISEZ-MOI.txt", "Rapport Kinseat. Journaux récents, état système et événements. Les secrets connus sont masqués automatiquement ; vérifiez le contenu avant partage. Le rapport ne contient ni captures ni mots de passe stockés.\n");
                AddText("etat.json", JsonSerializer.Serialize(state, new JsonSerializerOptions { WriteIndented = true }));
                var serviceLogs = Path.Combine(programData, "Kinseat", "logs");
                if (Directory.Exists(serviceLogs))
                    foreach (var source in Directory.EnumerateFiles(serviceLogs, "service-*.log*")
                        .OrderByDescending(File.GetLastWriteTimeUtc).Take(10))
                        AddFile(source, "service/" + Path.GetFileName(source));
                // Only known seat account names, and only simple directory components.
                var accountIndex = 0;
                foreach (var account in accounts.Distinct(StringComparer.OrdinalIgnoreCase))
                {
                    accountIndex++;
                    var alias = $"place-{accountIndex}";
                    if (account.Length is < 1 or > 128 || account is "." or ".." ||
                        account.IndexOfAny(Path.GetInvalidFileNameChars()) >= 0 || account.Contains('/') || account.Contains('\\')) continue;
                    var agent = Path.Combine(usersRoot, account, "AppData", "Local", "Kinseat");
                    AddFile(Path.Combine(agent, "agent.log"), "agents/" + alias + "/agent.log", usersRoot);
                    AddFile(Path.Combine(agent, "agent.log.1"), "agents/" + alias + "/agent.log.1", usersRoot);
                    var apollo = Path.Combine(programData, "MultiSeat", "apollo", account);
                    if (Directory.Exists(apollo))
                        foreach (var log in Directory.EnumerateFiles(apollo, "*.log").Take(5))
                            AddFile(log, "apollo/" + alias + "/" + Path.GetFileName(log), programData);
                }
                if (hostAccount is { Length: > 0 and <= 128 } && hostAccount is not ("." or "..") &&
                    hostAccount.IndexOfAny(Path.GetInvalidFileNameChars()) < 0 &&
                    !hostAccount.Contains('/') && !hostAccount.Contains('\\'))
                    AddFile(Path.Combine(usersRoot, hostAccount, "AppData", "Local", "Kinseat", "logs", "kinseat-app.log"),
                        "hote/kinseat-app.log", usersRoot);
                if (OperatingSystem.IsWindows())
                {
                    foreach (var channel in new[] { "Application", "System" })
                    {
                        try
                        {
                            using var reader = new EventLogReader(new EventLogQuery(channel, PathType.LogName) { ReverseDirection = true });
                            var lines = new StringBuilder();
                            for (var i = 0; i < 200; i++)
                            {
                                using var record = reader.ReadEvent();
                                if (record is null) break;
                                if (record.ProviderName is not ("MultiSeat.Service" or "MultiSeatService" or "Kinseat" or "Application Error")) continue;
                                lines.AppendLine($"{record.TimeCreated:O} {record.ProviderName} {record.Id}: {record.FormatDescription()}");
                            }
                            AddText("evenements/" + channel + ".txt", lines.ToString());
                        }
                        catch (Exception) { }
                    }
                }
            }
            return path;
        }
        catch { if (File.Exists(path)) File.Delete(path); throw; }
    }

    internal static object MachineDetails()
    {
        var gpus = new List<object>();
        if (OperatingSystem.IsWindows())
        {
            try
            {
                using var search = new ManagementObjectSearcher("SELECT Name, DriverVersion FROM Win32_VideoController");
                foreach (var gpu in search.Get())
                    gpus.Add(new { name = gpu["Name"]?.ToString(), driver = gpu["DriverVersion"]?.ToString() });
            }
            catch (Exception) { }
        }
        return new { version = typeof(AdminPipeServer).Assembly.GetName().Version?.ToString(),
            windows = Environment.OSVersion.VersionString, gpus };
    }
}
