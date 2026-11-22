namespace MultiSeat.Service.Library;

/// <summary>Conservative, bounded scan of game files; inaccessible folders and junctions are skipped.</summary>
public static class AntiCheatDetector
{
    public static bool Detect(string installDir)
    {
        if (string.IsNullOrWhiteSpace(installDir)) return false;
        return Scan(installDir, 0);
    }

    private static bool Scan(string directory, int depth)
    {
        try
        {
            if ((File.GetAttributes(directory) & FileAttributes.ReparsePoint) != 0) return false;
            foreach (var entry in Directory.EnumerateFileSystemEntries(directory))
            {
                try
                {
                    var name = Path.GetFileName(entry);
                    var attributes = File.GetAttributes(entry);
                    if ((attributes & FileAttributes.ReparsePoint) != 0) continue;
                    if (Matches(name)) return true;
                    if (depth < 2 && (attributes & FileAttributes.Directory) != 0 && Scan(entry, depth + 1)) return true;
                }
                catch (Exception ex) when (ex is IOException or UnauthorizedAccessException) { }
            }
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException) { }
        return false;
    }

    private static bool Matches(string name) =>
        name.StartsWith("EasyAntiCheat", StringComparison.OrdinalIgnoreCase) ||
        name.Equals("BattlEye", StringComparison.OrdinalIgnoreCase) ||
        name.StartsWith("BEService", StringComparison.OrdinalIgnoreCase) ||
        name.Equals("vgk.sys", StringComparison.OrdinalIgnoreCase) ||
        name.Equals("vgc.exe", StringComparison.OrdinalIgnoreCase) ||
        name.StartsWith("xigncode", StringComparison.OrdinalIgnoreCase) ||
        name.Equals("nProtect", StringComparison.OrdinalIgnoreCase) ||
        name.Equals("GameGuard", StringComparison.OrdinalIgnoreCase) ||
        name.StartsWith("mhyprot", StringComparison.OrdinalIgnoreCase) ||
        name.StartsWith("EAAntiCheat", StringComparison.OrdinalIgnoreCase);
}
