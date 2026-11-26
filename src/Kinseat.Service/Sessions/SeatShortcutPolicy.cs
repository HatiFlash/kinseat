using System.Collections.Concurrent;
using System.Security.AccessControl;
using System.Security.Cryptography;
using System.Security.Principal;
using System.Text;
using System.Text.Json;
using MultiSeat.Service.Accounts;

namespace MultiSeat.Service.Sessions;

/// <summary>Relocates host-tool public shortcuts out of public folders while managed seats exist.</summary>
internal static class SeatShortcutPolicy
{
    internal const int ManifestVersion = 2;
    internal const string StatePendingMove = "PendingMove";
    internal const string StateRelocated = "Relocated";
    internal const string StatePendingRestore = "PendingRestore";
    internal const string StateRestoreConflict = "RestoreConflict";

    private static readonly string[] AllowedNames =
    [
        "Kinseat Hôte.lnk",
        "Kinseat HÃ´te.lnk",
        "HidHide Configurator.lnk",
        "HidHide Configuration Client.lnk"
    ];

    private static readonly HashSet<string> AllowedNameSet = new(AllowedNames, StringComparer.OrdinalIgnoreCase);
    private static readonly StringComparison PathComparison = OperatingSystem.IsWindows()
        ? StringComparison.OrdinalIgnoreCase
        : StringComparison.Ordinal;

    internal sealed record LegacyShortcutEntry(string Path, string Sid);

    internal sealed record RelocatedShortcutEntry(
        string OriginalPath,
        string BackupPath,
        string State,
        string? OriginalSddl = null);

    internal sealed record ShortcutManifest(int Version, List<RelocatedShortcutEntry> Entries);

    internal sealed record PolicyPaths(string[] PublicRoots, string ManifestPath, string BackupDirectory);

    internal interface IShortcutAclHandler
    {
        void EnsureRestrictedDirectory(string directoryPath);
        string? CaptureAclAndStripLegacyDeny(string filePath, IReadOnlyCollection<string> legacyDenySids, ILogger logger);
        void RestrictBackupFile(string backupPath);
        void RestoreFileAcl(string filePath, string? originalSddl, ILogger logger);
        void RemoveLegacyDenyAce(string filePath, string sid, ILogger logger);
    }

    internal static readonly string InventoryPath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
        "Kinseat", "shutdown-rights", "shortcuts.json");

    internal static readonly string BackupDirectoryPath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
        "Kinseat", "shutdown-rights", "shortcuts-backup");

    private static readonly object Gate = new();
    private static readonly ConcurrentDictionary<string, byte> LoggedIgnoredReparsePaths = new(StringComparer.FromComparison(PathComparison));

    internal static readonly Environment.SpecialFolder[] DefaultPublicRootFolders =
    [
        Environment.SpecialFolder.CommonDesktopDirectory,
        Environment.SpecialFolder.CommonStartMenu,
        Environment.SpecialFolder.CommonPrograms
    ];

    internal static string[] GetDefaultPublicRoots() => NormalizeDistinctRoots(
        DefaultPublicRootFolders.Select(Environment.GetFolderPath));

    private static PolicyPaths DefaultPaths => new(
        GetDefaultPublicRoots(),
        InventoryPath,
        BackupDirectoryPath);

    internal static bool IsAllowedShortcutName(string? fileName) =>
        !string.IsNullOrWhiteSpace(fileName) && AllowedNameSet.Contains(fileName);

    internal static string[] SelectShortcuts(IEnumerable<string> paths) => paths
        .Where(path => IsAllowedShortcutName(Path.GetFileName(path)))
        .Distinct(StringComparer.OrdinalIgnoreCase)
        .ToArray();

    internal static void Apply(AccountManager accounts, ILogger logger)
    {
        if (!OperatingSystem.IsWindows()) return;
        lock (Gate)
        {
            var managedAccounts = accounts.ListManagedAccounts().Where(a => a.IsManaged).ToArray();
            var eligibleSids = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            var hasManagedSeats = false;
            foreach (var account in managedAccounts)
            {
                if (!accounts.IsManagedSeatAccount(account.Username)) continue;
                hasManagedSeats = true;
                var sid = AccountManager.TryResolveSid(account.Username, logger);
                if (sid is not null) eligibleSids.Add(sid);
            }
            ApplyCore(DefaultPaths, hasManagedSeats, eligibleSids, logger, WindowsAclHandler.Instance);
        }
    }

    internal static void ApplyCore(
        PolicyPaths paths,
        bool hasManagedSeats,
        IReadOnlyCollection<string> managedSeatSids,
        ILogger logger,
        IShortcutAclHandler? aclHandler = null)
    {
        aclHandler ??= OperatingSystem.IsWindows() ? WindowsAclHandler.Instance : NoOpAclHandler.Instance;

        if (IsReparsePoint(paths.ManifestPath))
        {
            LogIgnoredReparsePoint(logger, paths.ManifestPath, LogLevel.Warning, "Manifeste des raccourcis ignoré (lien symbolique/reparse point) : {Path}", paths.ManifestPath);
            return;
        }
        if (IsReparsePoint(paths.BackupDirectory))
        {
            LogIgnoredReparsePoint(logger, paths.BackupDirectory, LogLevel.Warning, "Répertoire de sauvegarde des raccourcis ignoré (lien symbolique/reparse point) : {Path}", paths.BackupDirectory);
            return;
        }

        var (manifest, migratedFromV1) = LoadOrMigrateManifest(paths, logger, aclHandler);
        if (manifest is null) return;

        if (!hasManagedSeats)
        {
            RestoreAll(paths, manifest, migratedFromV1, logger, aclHandler);
            return;
        }

        var manifestDirty = migratedFromV1;
        var skippedOriginals = new HashSet<string>(StringComparer.FromComparison(PathComparison));
        ReconcileExistingEntries(paths, manifest, ref manifestDirty, skippedOriginals, logger, aclHandler);

        var discovered = DiscoverTargetShortcuts(paths.PublicRoots, logger);
        foreach (var originalPath in discovered)
        {
            if (!ValidateOriginalPath(paths.PublicRoots, originalPath, out var normalizedOriginal)) continue;
            if (skippedOriginals.Contains(normalizedOriginal)) continue;
            if (HasReparsePointInSubpath(paths.PublicRoots, normalizedOriginal))
            {
                LogIgnoredReparsePoint(logger, normalizedOriginal, LogLevel.Warning, "Raccourci ignoré (reparse point détecté) : {Path}", normalizedOriginal);
                continue;
            }

            var existingIndex = manifest.Entries.FindIndex(e =>
                string.Equals(e.OriginalPath, normalizedOriginal, PathComparison));
            try
            {
                EnsureStorageReady(paths, aclHandler);
                var backupPath = existingIndex >= 0
                    ? manifest.Entries[existingIndex].BackupPath
                    : BuildBackupPath(paths.BackupDirectory, normalizedOriginal);

                if (!ValidateBackupPath(paths.BackupDirectory, normalizedOriginal, backupPath, out var normalizedBackup))
                {
                    logger.LogWarning("Chemin de sauvegarde invalide pour {OriginalPath} : {BackupPath}", normalizedOriginal, backupPath);
                    continue;
                }

                if (File.Exists(normalizedBackup))
                {
                    logger.LogWarning("Conflit de relocalisation du raccourci : {OriginalPath} et la sauvegarde {BackupPath} existent déjà ; les deux fichiers sont conservés sans écrasement",
                        normalizedOriginal, normalizedBackup);
                    var conflictEntry = existingIndex >= 0
                        ? manifest.Entries[existingIndex] with { OriginalPath = normalizedOriginal, BackupPath = normalizedBackup, State = StateRestoreConflict }
                        : new RelocatedShortcutEntry(normalizedOriginal, normalizedBackup, StateRestoreConflict);
                    if (existingIndex >= 0) manifest.Entries[existingIndex] = conflictEntry;
                    else manifest.Entries.Add(conflictEntry);
                    SaveManifest(paths, manifest, aclHandler);
                    manifestDirty = false;
                    continue;
                }

                var originalSddl = aclHandler.CaptureAclAndStripLegacyDeny(normalizedOriginal, managedSeatSids, logger);
                var pending = new RelocatedShortcutEntry(
                    normalizedOriginal,
                    normalizedBackup,
                    StatePendingMove,
                    originalSddl ?? (existingIndex >= 0 ? manifest.Entries[existingIndex].OriginalSddl : null));

                if (existingIndex >= 0) manifest.Entries[existingIndex] = pending;
                else manifest.Entries.Add(pending);
                SaveManifest(paths, manifest, aclHandler);
                manifestDirty = false;

                MoveToProtectedBackup(normalizedOriginal, normalizedBackup, aclHandler, logger);

                var relocated = pending with { State = StateRelocated };
                var idx = manifest.Entries.FindIndex(e =>
                    string.Equals(e.OriginalPath, normalizedOriginal, PathComparison));
                if (idx >= 0) manifest.Entries[idx] = relocated;
                SaveManifest(paths, manifest, aclHandler);
                logger.LogInformation("Raccourci public relocalisé : {OriginalPath} -> {BackupPath}", normalizedOriginal, normalizedBackup);
            }
            catch (Exception ex)
            {
                logger.LogWarning(ex, "Relocalisation du raccourci impossible : {Path}", normalizedOriginal);
            }
        }

        if (manifestDirty)
        {
            if (manifest.Entries.Count == 0)
                DeleteManifestIfExists(paths.ManifestPath);
            else
                SaveManifest(paths, manifest, aclHandler);
        }
    }

    private static (ShortcutManifest? Manifest, bool MigratedFromV1) LoadOrMigrateManifest(
        PolicyPaths paths,
        ILogger logger,
        IShortcutAclHandler aclHandler)
    {
        if (!File.Exists(paths.ManifestPath))
            return (new ShortcutManifest(ManifestVersion, []), false);

        string raw;
        try
        {
            raw = File.ReadAllText(paths.ManifestPath).Trim();
        }
        catch (Exception ex)
        {
            logger.LogWarning(ex, "Lecture du manifeste des raccourcis impossible : {Path}", paths.ManifestPath);
            return (null, false);
        }

        if (raw.StartsWith('['))
        {
            List<LegacyShortcutEntry>? legacy;
            try
            {
                legacy = JsonSerializer.Deserialize<List<LegacyShortcutEntry>>(raw);
            }
            catch (Exception ex)
            {
                logger.LogWarning(ex, "Manifeste v1 des raccourcis invalide : {Path}", paths.ManifestPath);
                return (null, false);
            }

            if (legacy is not null)
            {
                foreach (var item in legacy)
                {
                    if (!ValidateOriginalPath(paths.PublicRoots, item.Path, out var normalizedPath) ||
                        HasReparsePointInSubpath(paths.PublicRoots, normalizedPath))
                    {
                        logger.LogWarning("Entrée v1 ignorée (chemin hors périmètre ou lien symbolique) : {Path}", item.Path);
                        continue;
                    }
                    if (File.Exists(normalizedPath))
                        aclHandler.RemoveLegacyDenyAce(normalizedPath, item.Sid, logger);
                }
            }
            return (new ShortcutManifest(ManifestVersion, []), true);
        }

        try
        {
            var manifest = JsonSerializer.Deserialize<ShortcutManifest>(raw);
            if (manifest is null || manifest.Version != ManifestVersion || manifest.Entries is null)
            {
                logger.LogWarning("Manifeste des raccourcis invalide : {Path}", paths.ManifestPath);
                return (null, false);
            }
            return (manifest, false);
        }
        catch (Exception ex)
        {
            logger.LogWarning(ex, "Désérialisation du manifeste des raccourcis impossible : {Path}", paths.ManifestPath);
            return (null, false);
        }
    }

    private static void ReconcileExistingEntries(
        PolicyPaths paths,
        ShortcutManifest manifest,
        ref bool manifestDirty,
        HashSet<string> skippedOriginals,
        ILogger logger,
        IShortcutAclHandler aclHandler)
    {
        for (var i = manifest.Entries.Count - 1; i >= 0; i--)
        {
            var entry = manifest.Entries[i];
            if (!ValidateEntry(paths, entry, out var normalizedOriginal, out var normalizedBackup))
            {
                if (ValidateOriginalPath(paths.PublicRoots, entry.OriginalPath, out var rejectedOriginal))
                    skippedOriginals.Add(rejectedOriginal);
                logger.LogWarning("Entrée de manifeste refusée (chemin hors périmètre, hash invalide ou reparse point) : {Original} -> {Backup}",
                    entry.OriginalPath, entry.BackupPath);
                continue;
            }

            var origExists = File.Exists(normalizedOriginal);
            var bakExists = File.Exists(normalizedBackup);

            try
            {
                if (entry.State == StatePendingMove)
                {
                    if (!origExists && bakExists)
                    {
                        aclHandler.RestrictBackupFile(normalizedBackup);
                        manifest.Entries[i] = entry with { OriginalPath = normalizedOriginal, BackupPath = normalizedBackup, State = StateRelocated };
                        SaveManifest(paths, manifest, aclHandler);
                        manifestDirty = false;
                    }
                    else if (origExists && !bakExists)
                    {
                        EnsureStorageReady(paths, aclHandler);
                        MoveToProtectedBackup(normalizedOriginal, normalizedBackup, aclHandler, logger);
                        manifest.Entries[i] = entry with { OriginalPath = normalizedOriginal, BackupPath = normalizedBackup, State = StateRelocated };
                        SaveManifest(paths, manifest, aclHandler);
                        manifestDirty = false;
                    }
                    else if (origExists && bakExists)
                    {
                        logger.LogWarning("Conflit PendingMove du raccourci : {OriginalPath} et la sauvegarde {BackupPath} existent tous les deux ; les deux fichiers sont conservés sans écrasement",
                            normalizedOriginal, normalizedBackup);
                        manifest.Entries[i] = entry with { OriginalPath = normalizedOriginal, BackupPath = normalizedBackup, State = StateRestoreConflict };
                        SaveManifest(paths, manifest, aclHandler);
                        manifestDirty = false;
                        skippedOriginals.Add(normalizedOriginal);
                    }
                    else
                    {
                        logger.LogWarning("Entrée PendingMove orpheline nettoyée : {Original}", normalizedOriginal);
                        manifest.Entries.RemoveAt(i);
                        SaveManifest(paths, manifest, aclHandler);
                        manifestDirty = false;
                    }
                }
                else if (entry.State == StatePendingRestore || entry.State == StateRestoreConflict)
                {
                    if (bakExists && !origExists)
                    {
                        aclHandler.RestrictBackupFile(normalizedBackup);
                        manifest.Entries[i] = entry with { OriginalPath = normalizedOriginal, BackupPath = normalizedBackup, State = StateRelocated };
                        SaveManifest(paths, manifest, aclHandler);
                        manifestDirty = false;
                    }
                    else if (!bakExists && origExists)
                    {
                        manifest.Entries.RemoveAt(i);
                        SaveManifest(paths, manifest, aclHandler);
                        manifestDirty = false;
                    }
                    else if (!bakExists && !origExists)
                    {
                        manifest.Entries.RemoveAt(i);
                        SaveManifest(paths, manifest, aclHandler);
                        manifestDirty = false;
                    }
                    else
                    {
                        logger.LogWarning("Conflit conservé pour le raccourci : {OriginalPath} et {BackupPath} existent tous les deux ; aucun écrasement effectué",
                            normalizedOriginal, normalizedBackup);
                        if (entry.State != StateRestoreConflict)
                        {
                            manifest.Entries[i] = entry with { OriginalPath = normalizedOriginal, BackupPath = normalizedBackup, State = StateRestoreConflict };
                            SaveManifest(paths, manifest, aclHandler);
                            manifestDirty = false;
                        }
                        skippedOriginals.Add(normalizedOriginal);
                    }
                }
                else if (entry.State == StateRelocated && !bakExists && !origExists)
                {
                    logger.LogWarning("Sauvegarde de raccourci introuvable ; entrée nettoyée : {Backup}", normalizedBackup);
                    manifest.Entries.RemoveAt(i);
                    SaveManifest(paths, manifest, aclHandler);
                    manifestDirty = false;
                }
            }
            catch (Exception ex)
            {
                skippedOriginals.Add(normalizedOriginal);
                logger.LogWarning(ex, "Reprise d'état du raccourci impossible : {Original}", normalizedOriginal);
            }
        }
    }

    private static void RestoreAll(
        PolicyPaths paths,
        ShortcutManifest manifest,
        bool migratedFromV1,
        ILogger logger,
        IShortcutAclHandler aclHandler)
    {
        if (manifest.Entries.Count == 0)
        {
            if (migratedFromV1 || File.Exists(paths.ManifestPath))
                DeleteManifestIfExists(paths.ManifestPath);
            return;
        }

        for (var i = manifest.Entries.Count - 1; i >= 0; i--)
        {
            var entry = manifest.Entries[i];
            if (!ValidateEntry(paths, entry, out var normalizedOriginal, out var normalizedBackup))
            {
                logger.LogWarning("Restauration refusée (chemin hors périmètre ou reparse point) : {Original} -> {Backup}",
                    entry.OriginalPath, entry.BackupPath);
                continue;
            }

            var origExists = File.Exists(normalizedOriginal);
            var bakExists = File.Exists(normalizedBackup);

            try
            {
                if (entry.State == StatePendingMove)
                {
                    if (origExists && !bakExists)
                    {
                        manifest.Entries.RemoveAt(i);
                        SaveOrDeleteManifest(paths, manifest, aclHandler);
                        continue;
                    }
                    if (origExists && bakExists)
                    {
                        logger.LogWarning("Conflit PendingMove lors de la restauration : {OriginalPath} et la sauvegarde {BackupPath} existent tous les deux ; les deux fichiers sont conservés sans écrasement",
                            normalizedOriginal, normalizedBackup);
                        manifest.Entries[i] = entry with { OriginalPath = normalizedOriginal, BackupPath = normalizedBackup, State = StateRestoreConflict };
                        SaveManifest(paths, manifest, aclHandler);
                        continue;
                    }
                    if (!origExists && !bakExists)
                    {
                        manifest.Entries.RemoveAt(i);
                        SaveOrDeleteManifest(paths, manifest, aclHandler);
                        continue;
                    }
                }

                if (entry.State != StatePendingRestore && entry.State != StateRestoreConflict)
                {
                    manifest.Entries[i] = entry with { OriginalPath = normalizedOriginal, BackupPath = normalizedBackup, State = StatePendingRestore };
                    SaveManifest(paths, manifest, aclHandler);
                }

                if (!bakExists)
                {
                    if (origExists)
                    {
                        aclHandler.RestoreFileAcl(normalizedOriginal, entry.OriginalSddl, logger);
                    }
                    else
                    {
                        logger.LogWarning("Sauvegarde absente lors de la restauration : {Backup}", normalizedBackup);
                    }
                    manifest.Entries.RemoveAt(i);
                    SaveOrDeleteManifest(paths, manifest, aclHandler);
                    continue;
                }

                if (origExists)
                {
                    logger.LogWarning("Conflit de restauration du raccourci : {OriginalPath} existe déjà ; fichier conservé et sauvegarde maintenue dans {BackupPath}",
                        normalizedOriginal, normalizedBackup);
                    if (entry.State != StateRestoreConflict)
                    {
                        manifest.Entries[i] = entry with { OriginalPath = normalizedOriginal, BackupPath = normalizedBackup, State = StateRestoreConflict };
                        SaveManifest(paths, manifest, aclHandler);
                    }
                    continue;
                }

                var parentDir = Path.GetDirectoryName(normalizedOriginal)!;
                if (!EnsureDirectoryWithoutReparse(paths.PublicRoots, parentDir))
                {
                    logger.LogWarning("Restauration refusée (répertoire parent redirigé) : {Dir}", parentDir);
                    continue;
                }

                File.Move(normalizedBackup, normalizedOriginal, overwrite: false);
                aclHandler.RestoreFileAcl(normalizedOriginal, entry.OriginalSddl, logger);
                manifest.Entries.RemoveAt(i);
                SaveOrDeleteManifest(paths, manifest, aclHandler);
                logger.LogInformation("Raccourci public restauré : {OriginalPath}", normalizedOriginal);
            }
            catch (Exception ex)
            {
                logger.LogWarning(ex, "Restauration du raccourci impossible : {OriginalPath}", normalizedOriginal);
            }
        }

        if (manifest.Entries.Count == 0)
            DeleteManifestIfExists(paths.ManifestPath);
    }

    internal static string[] DiscoverTargetShortcuts(IEnumerable<string> publicRoots, ILogger logger)
    {
        var normalizedRoots = NormalizeDistinctRoots(publicRoots);
        var visitedDirs = new HashSet<string>(StringComparer.FromComparison(PathComparison));
        var results = new List<string>();
        foreach (var root in normalizedRoots)
        {
            if (!Directory.Exists(root)) continue;
            if (IsReparsePoint(root) || HasReparsePointAncestorInRoots(normalizedRoots, root))
            {
                LogIgnoredReparsePoint(logger, root, LogLevel.Warning, "Racine publique ignorée (reparse point) : {Root}", root);
                continue;
            }
            if (!visitedDirs.Add(root)) continue;

            var pending = new Stack<string>();
            pending.Push(root);
            while (pending.Count > 0)
            {
                var current = pending.Pop();
                IEnumerable<string> files;
                IEnumerable<string> subdirs;
                try
                {
                    var fileOptions = new EnumerationOptions { MatchCasing = MatchCasing.CaseInsensitive, RecurseSubdirectories = false };
                    files = Directory.EnumerateFiles(current, "*.lnk", fileOptions);
                    subdirs = Directory.EnumerateDirectories(current, "*", SearchOption.TopDirectoryOnly);
                }
                catch (Exception ex)
                {
                    logger.LogWarning(ex, "Parcours du répertoire public impossible : {Dir}", current);
                    continue;
                }

                foreach (var file in files)
                {
                    if (!IsAllowedShortcutName(Path.GetFileName(file))) continue;
                    if (IsReparsePoint(file))
                    {
                        LogIgnoredReparsePoint(logger, file, LogLevel.Warning, "Raccourci ignoré (reparse point) : {File}", file);
                        continue;
                    }
                    results.Add(Path.GetFullPath(file));
                }

                foreach (var subdir in subdirs)
                {
                    var normalizedSubdir = NormalizeDirectoryPath(subdir);
                    if (IsReparsePoint(normalizedSubdir))
                    {
                        if (IsKnownWindowsCompatibilityJunction(root, normalizedSubdir))
                            LogIgnoredReparsePoint(logger, normalizedSubdir, LogLevel.Debug, "Sous-répertoire public ignoré (jonction de compatibilité Windows) : {Dir}", subdir);
                        else
                            LogIgnoredReparsePoint(logger, normalizedSubdir, LogLevel.Warning, "Sous-répertoire public ignoré (reparse point) : {Dir}", subdir);
                        continue;
                    }
                    if (!visitedDirs.Add(normalizedSubdir)) continue;
                    pending.Push(normalizedSubdir);
                }
            }
        }
        return SelectShortcuts(results);
    }

    internal static bool ValidateEntry(
        PolicyPaths paths,
        RelocatedShortcutEntry entry,
        out string normalizedOriginal,
        out string normalizedBackup)
    {
        normalizedOriginal = string.Empty;
        normalizedBackup = string.Empty;
        if (!ValidateOriginalPath(paths.PublicRoots, entry.OriginalPath, out normalizedOriginal)) return false;
        if (!ValidateBackupPath(paths.BackupDirectory, normalizedOriginal, entry.BackupPath, out normalizedBackup)) return false;
        if (HasReparsePointInSubpath(paths.PublicRoots, normalizedOriginal)) return false;
        return true;
    }

    internal static bool ValidateOriginalPath(IEnumerable<string> publicRoots, string? path, out string normalized)
    {
        normalized = string.Empty;
        if (string.IsNullOrWhiteSpace(path) || !Path.IsPathRooted(path)) return false;
        try
        {
            var full = Path.GetFullPath(path);
            if (!IsAllowedShortcutName(Path.GetFileName(full))) return false;
            if (!IsStrictlyUnderAnyRoot(publicRoots, full)) return false;
            normalized = full;
            return true;
        }
        catch
        {
            return false;
        }
    }

    internal static bool ValidateBackupPath(
        string backupDirectory,
        string normalizedOriginalPath,
        string? path,
        out string normalized)
    {
        normalized = string.Empty;
        if (string.IsNullOrWhiteSpace(backupDirectory) ||
            string.IsNullOrWhiteSpace(normalizedOriginalPath) ||
            string.IsNullOrWhiteSpace(path) ||
            !Path.IsPathRooted(path))
            return false;
        try
        {
            var fullDir = Path.GetFullPath(backupDirectory).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
            var full = Path.GetFullPath(path);
            if (!IsStrictlyUnderRoot(fullDir, full)) return false;
            if (!string.Equals(Path.GetDirectoryName(full)?.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar), fullDir, PathComparison))
                return false;

            var expected = BuildBackupPath(fullDir, normalizedOriginalPath);
            var expectedFileName = Path.GetFileName(expected);
            var actualFileName = Path.GetFileName(full);
            if (actualFileName.Length != expectedFileName.Length || actualFileName.Length <= 17 || actualFileName[16] != '_')
                return false;
            if (!string.Equals(actualFileName[..16], expectedFileName[..16], StringComparison.OrdinalIgnoreCase))
                return false;
            if (!string.Equals(actualFileName[17..], expectedFileName[17..], PathComparison))
                return false;
            if (HasReparsePointInSubpath([fullDir], full))
                return false;

            normalized = full;
            return true;
        }
        catch
        {
            return false;
        }
    }

    /// <summary>
    /// Construit le chemin déterministe de sauvegarde :
    /// SHA-256 UTF-8 de <c>Path.GetFullPath(originalPath).ToUpperInvariant()</c> (canonicalisation Windows insensible à la casse),
    /// 16 premiers caractères hexadécimaux majuscules + '_' + nom de fichier d'origine, directement sous <paramref name="backupDirectory"/>.
    /// </summary>
    internal static string BuildBackupPath(string backupDirectory, string normalizedOriginalPath)
    {
        var canonicalOriginal = Path.GetFullPath(normalizedOriginalPath).ToUpperInvariant();
        var hash = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(canonicalOriginal)))[..16];
        var fileName = $"{hash}_{Path.GetFileName(normalizedOriginalPath)}";
        return Path.Combine(Path.GetFullPath(backupDirectory), fileName);
    }

    private static void MoveToProtectedBackup(
        string normalizedOriginal,
        string normalizedBackup,
        IShortcutAclHandler aclHandler,
        ILogger logger)
    {
        if (File.Exists(normalizedBackup))
            throw new IOException($"La sauvegarde existe déjà : {normalizedBackup}");

        var backupDir = Path.GetDirectoryName(normalizedBackup)!;
        var tempBackup = Path.Combine(backupDir, $".{Guid.NewGuid():N}.tmp");
        File.Move(normalizedOriginal, tempBackup, overwrite: false);
        try
        {
            aclHandler.RestrictBackupFile(tempBackup);
            File.Move(tempBackup, normalizedBackup, overwrite: false);
        }
        catch (Exception ex)
        {
            if (File.Exists(tempBackup))
            {
                if (!File.Exists(normalizedOriginal))
                {
                    try
                    {
                        File.Move(tempBackup, normalizedOriginal, overwrite: false);
                    }
                    catch (Exception rollbackEx)
                    {
                        logger.LogWarning(rollbackEx,
                            "Retour arrière impossible après échec de sécurisation de la sauvegarde ; fichier temporaire conservé : {TempPath}",
                            tempBackup);
                    }
                }
                else
                {
                    logger.LogWarning(
                        "Retour arrière impossible sans écrasement car {OriginalPath} existe déjà ; fichier temporaire conservé : {TempPath}",
                        normalizedOriginal, tempBackup);
                }
            }
            throw new IOException($"Sécurisation ou déplacement final de la sauvegarde impossible pour {normalizedOriginal}", ex);
        }
    }

    private static string NormalizeDirectoryPath(string path)
    {
        var full = Path.GetFullPath(path);
        var trimmed = full.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
        return string.IsNullOrEmpty(trimmed) ? full : trimmed;
    }

    private static string[] NormalizeDistinctRoots(IEnumerable<string> roots) => roots
        .Where(r => !string.IsNullOrWhiteSpace(r))
        .Select(NormalizeDirectoryPath)
        .Where(r => !string.IsNullOrWhiteSpace(r))
        .Distinct(StringComparer.FromComparison(PathComparison))
        .OrderBy(r => r.Length)
        .ToArray();

    private static bool HasReparsePointAncestorInRoots(IEnumerable<string> normalizedRoots, string normalizedTarget)
    {
        foreach (var ancestor in normalizedRoots)
        {
            if (!IsStrictlyUnderRoot(ancestor, normalizedTarget)) continue;
            if (IsReparsePoint(ancestor)) return true;
            var relative = Path.GetRelativePath(ancestor, normalizedTarget);
            var current = ancestor;
            foreach (var segment in relative.Split([Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar], StringSplitOptions.RemoveEmptyEntries))
            {
                current = Path.Combine(current, segment);
                if (IsReparsePoint(current)) return true;
            }
        }
        return false;
    }

    private static bool IsStrictlyUnderAnyRoot(IEnumerable<string> roots, string fullPath)
    {
        foreach (var root in roots)
        {
            if (string.IsNullOrWhiteSpace(root)) continue;
            var fullRoot = NormalizeDirectoryPath(root);
            if (IsStrictlyUnderRoot(fullRoot, fullPath)) return true;
        }
        return false;
    }

    private static bool IsStrictlyUnderRoot(string normalizedRoot, string fullPath)
    {
        var prefix = normalizedRoot.EndsWith(Path.DirectorySeparatorChar)
            ? normalizedRoot
            : normalizedRoot + Path.DirectorySeparatorChar;
        return fullPath.StartsWith(prefix, PathComparison) && fullPath.Length > prefix.Length;
    }

    private static bool HasReparsePointInSubpath(IEnumerable<string> roots, string fullTarget)
    {
        var matchedAny = false;
        foreach (var root in roots)
        {
            if (string.IsNullOrWhiteSpace(root)) continue;
            var fullRoot = NormalizeDirectoryPath(root);
            if (!IsStrictlyUnderRoot(fullRoot, fullTarget)) continue;
            matchedAny = true;
            if (IsReparsePoint(fullRoot)) return true;
            var relative = Path.GetRelativePath(fullRoot, fullTarget);
            var current = fullRoot;
            foreach (var segment in relative.Split([Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar], StringSplitOptions.RemoveEmptyEntries))
            {
                current = Path.Combine(current, segment);
                if (IsReparsePoint(current)) return true;
            }
        }
        return !matchedAny;
    }

    private static bool EnsureDirectoryWithoutReparse(IEnumerable<string> roots, string directoryPath)
    {
        var fullDir = NormalizeDirectoryPath(directoryPath);
        foreach (var fullRoot in NormalizeDistinctRoots(roots))
        {
            if (string.Equals(fullDir, fullRoot, PathComparison))
            {
                if (IsReparsePoint(fullRoot)) return false;
                Directory.CreateDirectory(fullRoot);
                return !IsReparsePoint(fullRoot);
            }
            if (!IsStrictlyUnderRoot(fullRoot, fullDir)) continue;
            if (IsReparsePoint(fullRoot)) return false;
            var relative = Path.GetRelativePath(fullRoot, fullDir);
            var current = fullRoot;
            foreach (var segment in relative.Split([Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar], StringSplitOptions.RemoveEmptyEntries))
            {
                current = Path.Combine(current, segment);
                if (IsReparsePoint(current)) return false;
                if (!Directory.Exists(current)) Directory.CreateDirectory(current);
                if (IsReparsePoint(current)) return false;
            }
            return true;
        }
        return false;
    }

    private static void LogIgnoredReparsePoint(
        ILogger logger,
        string path,
        LogLevel level,
        string messageTemplate,
        string displayPath)
    {
        if (!TryMarkReparsePathLogged(path))
            return;

        logger.Log(level, messageTemplate, displayPath);
    }

    private static bool TryMarkReparsePathLogged(string path)
    {
        string key;
        try { key = NormalizeDirectoryPath(path); }
        catch { key = path; }
        return LoggedIgnoredReparsePaths.TryAdd(key, 0);
    }

    internal static bool IsKnownWindowsCompatibilityJunction(
        string publicRoot,
        string normalizedSubdir,
        FileAttributes? attributesOverride = null)
    {
        try
        {
            var attrs = attributesOverride ?? File.GetAttributes(normalizedSubdir);
            if ((attrs & (FileAttributes.Hidden | FileAttributes.System)) == (FileAttributes.Hidden | FileAttributes.System))
                return true;
        }
        catch
        {
            // Fall through to link target inspection.
        }

        try
        {
            var normalizedRoot = NormalizeDirectoryPath(publicRoot);
            var target = new DirectoryInfo(normalizedSubdir).ResolveLinkTarget(returnFinalTarget: false);
            if (target is null)
                return false;

            var normalizedTarget = NormalizeDirectoryPath(target.FullName);
            return !string.Equals(normalizedTarget, normalizedSubdir, PathComparison) &&
                Directory.Exists(normalizedTarget) &&
                IsStrictlyUnderRoot(normalizedRoot, normalizedTarget);
        }
        catch
        {
            return false;
        }
    }

    private static bool IsReparsePoint(string path)
    {
        try
        {
            var fi = new FileInfo(path);
            if (fi.LinkTarget is not null) return true;
            var di = new DirectoryInfo(path);
            if (di.LinkTarget is not null) return true;
            if (!fi.Exists && !di.Exists) return false;
            return (File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0;
        }
        catch (FileNotFoundException) { return false; }
        catch (DirectoryNotFoundException) { return false; }
    }

    private static void EnsureStorageReady(PolicyPaths paths, IShortcutAclHandler aclHandler)
    {
        var manifestDir = Path.GetDirectoryName(paths.ManifestPath)!;
        aclHandler.EnsureRestrictedDirectory(manifestDir);
        aclHandler.EnsureRestrictedDirectory(paths.BackupDirectory);
    }

    private static void SaveManifest(PolicyPaths paths, ShortcutManifest manifest, IShortcutAclHandler aclHandler)
    {
        var manifestDir = Path.GetDirectoryName(paths.ManifestPath)!;
        aclHandler.EnsureRestrictedDirectory(manifestDir);
        var tmp = paths.ManifestPath + ".tmp";
        File.WriteAllText(tmp, JsonSerializer.Serialize(manifest));
        aclHandler.RestrictBackupFile(tmp);
        File.Move(tmp, paths.ManifestPath, overwrite: true);
        aclHandler.RestrictBackupFile(paths.ManifestPath);
    }

    private static void SaveOrDeleteManifest(PolicyPaths paths, ShortcutManifest manifest, IShortcutAclHandler aclHandler)
    {
        if (manifest.Entries.Count == 0)
            DeleteManifestIfExists(paths.ManifestPath);
        else
            SaveManifest(paths, manifest, aclHandler);
    }

    private static void DeleteManifestIfExists(string manifestPath)
    {
        if (File.Exists(manifestPath)) File.Delete(manifestPath);
        var tmp = manifestPath + ".tmp";
        if (File.Exists(tmp)) File.Delete(tmp);
    }

    private sealed class NoOpAclHandler : IShortcutAclHandler
    {
        internal static readonly NoOpAclHandler Instance = new();
        public void EnsureRestrictedDirectory(string directoryPath) => Directory.CreateDirectory(directoryPath);
        public string? CaptureAclAndStripLegacyDeny(string filePath, IReadOnlyCollection<string> legacyDenySids, ILogger logger) => null;
        public void RestrictBackupFile(string backupPath) { }
        public void RestoreFileAcl(string filePath, string? originalSddl, ILogger logger) { }
        public void RemoveLegacyDenyAce(string filePath, string sid, ILogger logger) { }
    }

    private sealed class WindowsAclHandler : IShortcutAclHandler
    {
        internal static readonly WindowsAclHandler Instance = new();

        public void EnsureRestrictedDirectory(string directoryPath)
        {
            var dir = new DirectoryInfo(directoryPath);
            dir.Create();
            if ((dir.Attributes & FileAttributes.ReparsePoint) != 0)
                throw new IOException("Répertoire de sauvegarde redirigé");
            if (!OperatingSystem.IsWindows()) return;
            var acl = dir.GetAccessControl();
            acl.SetAccessRuleProtection(true, false);
            foreach (FileSystemAccessRule rule in acl.GetAccessRules(true, false, typeof(SecurityIdentifier)))
                acl.RemoveAccessRuleSpecific(rule);
            foreach (var principal in new[] { ShutdownRightPolicy.AdminsSid, "S-1-5-18" })
                acl.AddAccessRule(new FileSystemAccessRule(
                    new SecurityIdentifier(principal),
                    FileSystemRights.FullControl,
                    InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit,
                    PropagationFlags.None,
                    AccessControlType.Allow));
            dir.SetAccessControl(acl);
        }

        public string? CaptureAclAndStripLegacyDeny(string filePath, IReadOnlyCollection<string> legacyDenySids, ILogger logger)
        {
            if (!OperatingSystem.IsWindows()) return null;
            var file = new FileInfo(filePath);
            var acl = file.GetAccessControl();
            var changed = false;
            if (legacyDenySids.Count > 0)
            {
                var sidSet = legacyDenySids.ToHashSet(StringComparer.OrdinalIgnoreCase);
                foreach (var rule in acl.GetAccessRules(true, false, typeof(SecurityIdentifier))
                    .OfType<FileSystemAccessRule>()
                    .Where(r => !r.IsInherited &&
                        r.AccessControlType == AccessControlType.Deny &&
                        r.FileSystemRights == FileSystemRights.Read &&
                        sidSet.Contains(r.IdentityReference.Value))
                    .ToArray())
                {
                    acl.RemoveAccessRuleSpecific(rule);
                    changed = true;
                }
                if (changed) file.SetAccessControl(acl);
            }
            return acl.GetSecurityDescriptorSddlForm(AccessControlSections.Access);
        }

        public void RestrictBackupFile(string backupPath)
        {
            if (!OperatingSystem.IsWindows()) return;
            var file = new FileInfo(backupPath);
            var acl = file.GetAccessControl();
            acl.SetAccessRuleProtection(true, false);
            foreach (FileSystemAccessRule rule in acl.GetAccessRules(true, false, typeof(SecurityIdentifier)))
                acl.RemoveAccessRuleSpecific(rule);
            foreach (var principal in new[] { ShutdownRightPolicy.AdminsSid, "S-1-5-18" })
                acl.AddAccessRule(new FileSystemAccessRule(
                    new SecurityIdentifier(principal),
                    FileSystemRights.FullControl,
                    AccessControlType.Allow));
            file.SetAccessControl(acl);
        }

        public void RestoreFileAcl(string filePath, string? originalSddl, ILogger logger)
        {
            if (!OperatingSystem.IsWindows()) return;
            var file = new FileInfo(filePath);
            var acl = file.GetAccessControl();
            if (!string.IsNullOrWhiteSpace(originalSddl))
            {
                try
                {
                    acl.SetSecurityDescriptorSddlForm(originalSddl, AccessControlSections.Access);
                    file.SetAccessControl(acl);
                    return;
                }
                catch (Exception ex)
                {
                    logger.LogWarning(ex, "Restauration ACL SDDL impossible pour {Path}, réactivation de l'héritage", filePath);
                }
            }
            acl.SetAccessRuleProtection(false, false);
            foreach (FileSystemAccessRule rule in acl.GetAccessRules(true, false, typeof(SecurityIdentifier)))
                acl.RemoveAccessRuleSpecific(rule);
            file.SetAccessControl(acl);
        }

        public void RemoveLegacyDenyAce(string filePath, string sid, ILogger logger)
        {
            if (!OperatingSystem.IsWindows()) return;
            try
            {
                var file = new FileInfo(filePath);
                var acl = file.GetAccessControl();
                acl.RemoveAccessRuleSpecific(new FileSystemAccessRule(
                    new SecurityIdentifier(sid), FileSystemRights.Read, AccessControlType.Deny));
                file.SetAccessControl(acl);
            }
            catch (Exception ex)
            {
                logger.LogWarning(ex, "Nettoyage ACL v1 du raccourci impossible : {Path}", filePath);
            }
        }
    }
}
