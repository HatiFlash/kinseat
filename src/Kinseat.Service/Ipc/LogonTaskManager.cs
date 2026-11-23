using System.Diagnostics;
using System.Security.Cryptography;
using System.Text;
using System.Xml.Linq;

namespace MultiSeat.Service.Ipc;

/// <summary>Only tasks explicitly selected by the host operator are ever edited.</summary>
internal static class LogonTaskManager
{
    private static readonly XNamespace Ns = "http://schemas.microsoft.com/windows/2004/02/mit/task";
    private static readonly string BackupDir = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData), "MultiSeat", "task-backups");

    internal static bool ValidPath(string path, string name) =>
        path.StartsWith('\\') && path.EndsWith('\\') && path.Length <= 256 &&
        !path.StartsWith(@"\Microsoft\", StringComparison.OrdinalIgnoreCase) &&
        path.Split('\\').All(p => p != "." && p != ".." && !p.Any(char.IsControl)) &&
        name is { Length: >= 1 and <= 128 } && name is not ("." or "..") &&
        name.IndexOfAny(['\\', '/', '"', ':', '*', '?', '<', '>', '|']) < 0 && !name.Any(char.IsControl);

    /// <param name="restrictedByKinseat">A Kinseat backup exists for this task: Task Scheduler may rewrite the
    /// trigger UserId (SID vs DOMAIN\\name), so the backup is the reliable proof of our own restriction.</param>
    internal static (string? Execute, string? RunAs, bool Restricted) Detect(string xml, bool restrictedByKinseat = false)
    {
        var doc = XDocument.Parse(xml, LoadOptions.None);
        var principal = doc.Descendants(Ns + "Principal").FirstOrDefault(p =>
            p.Element(Ns + "LogonType")?.Value is "InteractiveToken" or "Interactive");
        var runAs = principal?.Element(Ns + "UserId")?.Value;
        if (string.IsNullOrWhiteSpace(runAs)) return (null, null, false);
        var triggers = doc.Descendants(Ns + "LogonTrigger").ToArray();
        if (triggers.Length == 0) return (null, null, false);
        var execute = doc.Descendants(Ns + "Exec").Select(e => e.Element(Ns + "Command")?.Value)
            .FirstOrDefault(v => !string.IsNullOrWhiteSpace(v));
        var any = triggers.Any(t => string.IsNullOrWhiteSpace(t.Element(Ns + "UserId")?.Value));
        var restricted = !any && (restrictedByKinseat || triggers.All(t => SameAccount(t.Element(Ns + "UserId")?.Value, runAs)));
        return (any || restricted ? execute : null, runAs, restricted);
    }

    internal static string ExeName(string? execute)
    {
        if (string.IsNullOrWhiteSpace(execute)) return "";
        var command = Environment.ExpandEnvironmentVariables(execute.Trim());
        var path = command.StartsWith('"') ? command[1..].Split('"')[0]
            : command.Contains(".exe", StringComparison.OrdinalIgnoreCase)
                ? command[..(command.IndexOf(".exe", StringComparison.OrdinalIgnoreCase) + 4)]
                : command.Split(' ', StringSplitOptions.RemoveEmptyEntries)[0];
        return path.Replace('/', '\\').Split('\\').Last();
    }

    internal static string DisplayName(string taskName, string exeName, string? description, string? product)
    {
        foreach (var candidate in new[] { description, product, Path.GetFileNameWithoutExtension(exeName), taskName })
        {
            var clean = candidate?.Trim().Trim('"');
            if (!string.IsNullOrWhiteSpace(clean) && clean.IndexOfAny(['\\', '/', '"']) < 0)
                return clean;
        }
        return "Programme";
    }

    internal static string Kind(string taskName, string exeName, string xml)
    {
        var combined = taskName + " " + exeName;
        string[] overlays = ["hwinfo", "msiafterburner", "afterburner", "rtss", "rivatuner", "aida64",
            "nzxt", "icue", "armoury", "signalrgb", "wallpaper engine", "wallpaperengine", "rainmeter"];
        if (overlays.Any(v => combined.Contains(v, StringComparison.OrdinalIgnoreCase))) return "overlay";
        if (new[] { "sale", "update", "updater" }.Any(v => combined.Contains(v, StringComparison.OrdinalIgnoreCase)))
            return "oneTime";
        var doc = XDocument.Parse(xml, LoadOptions.None);
        return doc.Descendants(Ns + "TimeTrigger").Any(t => !t.Descendants(Ns + "Repetition").Any())
            ? "oneTime" : "other";
    }

    /// <summary>Listed: opens at every logon (not restricted), or restricted by Kinseat (restorable).</summary>
    internal static bool Listed(bool restricted, bool hasKinseatBackup) => !restricted || hasKinseatBackup;

    /// <summary>Compares two task account references, accepting SID or DOMAIN\\name forms.</summary>
    internal static bool SameAccount(string? a, string? b)
    {
        if (string.IsNullOrWhiteSpace(a) || string.IsNullOrWhiteSpace(b)) return false;
        if (string.Equals(a, b, StringComparison.OrdinalIgnoreCase)) return true;
        var sa = ToSid(a); var sb = ToSid(b);
        return sa is not null && string.Equals(sa, sb, StringComparison.OrdinalIgnoreCase);
    }

    private static string? ToSid(string account)
    {
        if (account.StartsWith("S-1-", StringComparison.OrdinalIgnoreCase)) return account;
        if (!OperatingSystem.IsWindows()) return null;
        try { return ((System.Security.Principal.SecurityIdentifier)new System.Security.Principal.NTAccount(account)
            .Translate(typeof(System.Security.Principal.SecurityIdentifier))).Value; }
        catch (Exception) { return null; }
    }

    /// <summary>Readable account for the UI (DOMAIN\\name instead of a SID when it resolves).</summary>
    internal static string? DisplayAccount(string? account)
    {
        if (string.IsNullOrWhiteSpace(account) || !account.StartsWith("S-1-", StringComparison.OrdinalIgnoreCase)
            || !OperatingSystem.IsWindows()) return account;
        try { return new System.Security.Principal.SecurityIdentifier(account)
            .Translate(typeof(System.Security.Principal.NTAccount)).Value; }
        catch (Exception) { return account; }
    }

    private static string TaskName(string path, string name) => path + name;
    private static string BackupPath(string path, string name) => Path.Combine(BackupDir,
        Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(TaskName(path, name)))) + ".xml");

    internal static string ArchiveStaleBackup(string backupFilePath, DateTimeOffset utcNow)
    {
        var timestamp = utcNow.UtcDateTime.ToString("yyyyMMdd'T'HHmmss'Z'", System.Globalization.CultureInfo.InvariantCulture);
        var newPath = $"{backupFilePath}.old-{timestamp}";
        var counter = 0;
        while (File.Exists(newPath))
        {
            counter++;
            newPath = $"{backupFilePath}.old-{timestamp}-{counter}";
        }
        File.Move(backupFilePath, newPath);
        return newPath;
    }

    private static async Task<string> RunAsync(CancellationToken ct, params string[] arguments)
    {
        using var process = new Process { StartInfo = new ProcessStartInfo(Path.Combine(Environment.SystemDirectory, "schtasks.exe"))
        { UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true } };
        foreach (var argument in arguments) process.StartInfo.ArgumentList.Add(argument);
        process.Start();
        var output = process.StandardOutput.ReadToEndAsync(ct);
        var error = process.StandardError.ReadToEndAsync(ct);
        await process.WaitForExitAsync(ct);
        var text = await output;
        if (process.ExitCode != 0) throw new IOException($"schtasks exited {process.ExitCode}: {await error}");
        return text;
    }

    private static Task<string> ExportAsync(string path, string name, CancellationToken ct) =>
        RunAsync(ct, "/Query", "/TN", TaskName(path, name), "/XML");

    internal static async Task<object[]> ListAsync(CancellationToken ct)
    {
        // /FO CSV provides path/name without depending on localized column names.
        var csv = await RunAsync(ct, "/Query", "/FO", "CSV", "/NH");
        var names = ParseTaskNames(csv);
        var found = new List<object>();
        foreach (var full in names)
        {
            var split = full.LastIndexOf('\\');
            var path = full[..(split + 1)];
            var name = full[(split + 1)..];
            if (!ValidPath(path, name)) continue;
            try
            {
                var hasBackup = File.Exists(BackupPath(path, name));
                var xml = await ExportAsync(path, name, ct);
                var (execute, runAs, restricted) = Detect(xml, hasBackup);
                // A task already bound to one user without a Kinseat backup (e.g. per-user OneDrive)
                // never opens on the main screen and cannot be « restored »: not listed.
                if (execute is not null && Listed(restricted, hasBackup))
                {
                    var exeName = ExeName(execute);
                    string? description = null, product = null;
                    try
                    {
                        var exePath = Environment.ExpandEnvironmentVariables(execute.Trim().TrimStart('"').Split('"')[0]);
                        if (File.Exists(exePath))
                        {
                            var version = FileVersionInfo.GetVersionInfo(exePath);
                            description = version.FileDescription;
                            product = version.ProductName;
                        }
                    }
                    catch (Exception) { /* Version metadata is optional. */ }
                    found.Add(new { path, name, execute, runAs = DisplayAccount(runAs), restricted,
                        displayName = DisplayName(name, exeName, description, product), exeName, kind = Kind(name, exeName, xml) });
                }
            }
            catch (Exception ex) when (ex is IOException or System.Xml.XmlException) { /* unreadable task */ }
        }
        return found.ToArray();
    }

    internal static string[] ParseTaskNames(string csv) => csv.Split('\n', StringSplitOptions.RemoveEmptyEntries)
        .Select(line => line.Trim().TrimStart('"').Split("\",\"", 2)[0].Trim('"', '\r'))
        .Where(full => full.StartsWith('\\') && full.LastIndexOf('\\') is >= 0 and < 255)
        .Distinct(StringComparer.OrdinalIgnoreCase).ToArray();

    private static async Task<bool> ExistsAsync(string path, string name, CancellationToken ct)
    {
        var csv = await RunAsync(ct, "/Query", "/FO", "CSV", "/NH");
        return ParseTaskNames(csv).Any(full =>
            string.Equals(full, TaskName(path, name), StringComparison.OrdinalIgnoreCase));
    }

    internal static async Task<object> RestrictAsync(string path, string name, CancellationToken ct)
    {
        if (!ValidPath(path, name))
            throw new AdminFault("invalid_params", "Nom de tâche invalide");
        if (!await ExistsAsync(path, name, ct))
            throw new AdminFault("not_found", "Tâche planifiée introuvable");
        var xml = await ExportAsync(path, name, ct);

        var (execute, runAs, restricted) = Detect(xml);
        if (execute is null || restricted || string.IsNullOrWhiteSpace(runAs))
            throw new AdminFault("invalid_params", "La tâche doit avoir un déclencheur à l'ouverture de session de tout utilisateur");
        var document = XDocument.Parse(xml);
        foreach (var trigger in document.Descendants(Ns + "LogonTrigger")
                     .Where(t => string.IsNullOrWhiteSpace(t.Element(Ns + "UserId")?.Value)))
            trigger.Add(new XElement(Ns + "UserId", runAs));
        Directory.CreateDirectory(BackupDir);
        var backupPath = BackupPath(path, name);
        // The task is not restricted any more (checked above): its owner rewrote it after an
        // earlier restriction. Keep the stale copy under an archive name, never delete it.
        if (File.Exists(backupPath)) ArchiveStaleBackup(backupPath, DateTimeOffset.UtcNow);
        XDocument.Parse(xml).Save(backupPath);
        try { MultiSeat.Service.Storage.SecureFile.RestrictToSystemAndAdmins(backupPath); }
        catch { File.Delete(backupPath); throw; }
        var candidate = backupPath + ".candidate";
        try
        {
            document.Save(candidate);
            MultiSeat.Service.Storage.SecureFile.RestrictToSystemAndAdmins(candidate);
            await RunAsync(ct, "/Create", "/TN", TaskName(path, name), "/XML", candidate, "/F");
            return new { ok = true };
        }
        finally { File.Delete(candidate); }
    }

    internal static async Task<object> RestoreAsync(string path, string name, CancellationToken ct)
    {
        if (!ValidPath(path, name))
            throw new AdminFault("invalid_params", "Nom de tâche invalide");
        if (!await ExistsAsync(path, name, ct))
            throw new AdminFault("not_found", "Tâche planifiée introuvable");
        var backup = BackupPath(path, name);
        if (!File.Exists(backup)) throw new AdminFault("not_found", "Sauvegarde de tâche introuvable");
        // Do not overwrite a task subsequently changed by its owner.
        var (_, _, restricted) = Detect(await ExportAsync(path, name, ct), restrictedByKinseat: true);
        if (!restricted) throw new AdminFault("conflict", "La tâche a été modifiée depuis sa restriction");
        await RunAsync(ct, "/Create", "/TN", TaskName(path, name), "/XML", backup, "/F");
        File.Delete(backup);
        return new { ok = true };
    }
}
