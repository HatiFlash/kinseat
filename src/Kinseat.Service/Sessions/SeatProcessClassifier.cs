namespace MultiSeat.Service.Sessions;

/// <summary>Conservative process classifier. A name match alone never authorizes termination;
/// callers must also verify the process's live SessionId against the seat's SessionId.</summary>
internal static class SeatProcessClassifier
{
    private static readonly HashSet<string> Protected = new(StringComparer.OrdinalIgnoreCase)
    {
        "explorer.exe", "sunshine.exe", "kinseat.exe", "kinseat-agent.exe", "multiseat.service.exe",
        "csrss.exe", "winlogon.exe", "services.exe", "lsass.exe", "svchost.exe",
        "dwm.exe", "securityhealthsystray.exe", "securityhealthservice.exe",
        "msmpeng.exe", "nissrv.exe", "senseir.exe", "sensesampleuploader.exe",
        "windefend.exe", "avp.exe", "mbamservice.exe", "savservice.exe"
    };

    private static readonly HashSet<string> Vendors = new(StringComparer.OrdinalIgnoreCase)
    {
        "icue.exe", "armourycrate.exe", "armourycrate.userSessionHelper.exe",
        "msiafterburner.exe", "rtss.exe", "hwinfo64.exe", "hwinfo32.exe",
        "radeonsoftware.exe", "amdrsserv.exe", "amdrssrcext.exe", "nvidia app.exe", "nvcontainer.exe",
        "lghub.exe", "lghub_agent.exe", "razer synapse 3.exe", "razercentral.exe"
    };

    internal static bool IsProtected(string name) => Protected.Contains(Path.GetFileName(name));

    internal static bool IsSecurityStartup(string name) =>
        IsProtected(name) && name is not ("explorer.exe" or "sunshine.exe" or "kinseat.exe") ||
        name.Contains("securityhealth", StringComparison.OrdinalIgnoreCase) ||
        name.Contains("defender", StringComparison.OrdinalIgnoreCase) ||
        name.Contains("antivirus", StringComparison.OrdinalIgnoreCase) ||
        name.Contains("antimalware", StringComparison.OrdinalIgnoreCase) ||
        name.Contains("crowdstrike", StringComparison.OrdinalIgnoreCase) ||
        name.Contains("sentinelone", StringComparison.OrdinalIgnoreCase) ||
        name.Contains("sophos", StringComparison.OrdinalIgnoreCase) ||
        name.Contains("bitdefender", StringComparison.OrdinalIgnoreCase) ||
        name.Contains("kaspersky", StringComparison.OrdinalIgnoreCase);

    internal static string? Classify(string name, int processSessionId, int seatSessionId,
        bool startup, bool vendorUtilities, bool closeWindhawk, bool gameProcess = false)
    {
        if (seatSessionId < 1 || processSessionId != seatSessionId || gameProcess ||
            IsProtected(name) || IsSecurityStartup(name)) return null;
        var file = Path.GetFileName(name);
        if (closeWindhawk && file.Equals("windhawk.exe", StringComparison.OrdinalIgnoreCase)) return "windhawk";
        if (vendorUtilities && Vendors.Contains(file)) return "vendor";
        return startup ? "startup" : null;
    }
}
