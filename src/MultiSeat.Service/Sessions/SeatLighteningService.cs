using System.ComponentModel;
using System.Diagnostics;
using System.Management;
using System.Runtime.InteropServices;
using Microsoft.Win32;
using MultiSeat.Service.Accounts;
using MultiSeat.Service.Interop;
using MultiSeat.Shared.Models;

namespace MultiSeat.Service.Sessions;

/// <summary>Per-account registry policy and per-session, conservative process cleanup.</summary>
public sealed class SeatLighteningService(ILogger<SeatLighteningService> logger, IServiceProvider services) : BackgroundService
{
    private const string Explorer = @"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";
    private const string Approved = @"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
    private const string Owned = @"Software\Kinseat\Lightening\BlockedStartup";
    private const string Run = @"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
    internal static readonly byte[] Disabled = [3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    internal static readonly byte[] Enabled = [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    private readonly object _gate = new();
    private readonly Dictionary<Guid, List<StoppedProcess>> _stopped = [];
    private readonly Dictionary<Guid, Dictionary<string, int>> _vendorStops = [];
    private readonly HashSet<(Guid Seat, string Name)> _vendorLimitLogged = [];

    // Only three stops per vendor executable per seat. A restart loop must not cause endless kills/logs.
    internal static bool MayStopVendor(int priorStops) => priorStops < 3;
    private readonly Dictionary<Guid, (int Session, DateTimeOffset Since)> _windows = [];
    private readonly Dictionary<string, HashSet<string>> _startupBySid = new(StringComparer.OrdinalIgnoreCase);
    private readonly Dictionary<string, string[]> _blockedBySid = new(StringComparer.OrdinalIgnoreCase);

    public sealed record StoppedProcess(string Name, int Pid, string Category, DateTimeOffset At);
    internal sealed record ProcessSnapshot(int Pid, int SessionId, string Name, int ParentPid);

    internal static IReadOnlyDictionary<string, string> StartupEntries()
    {
        var entries = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        foreach (var (view, target) in new[] { (RegistryView.Registry64, "Run"), (RegistryView.Registry32, "Run32") })
        {
            using var machine = RegistryKey.OpenBaseKey(RegistryHive.LocalMachine, view);
            using var run = machine.OpenSubKey(Run);
            if (run is null) continue;
            foreach (var name in run.GetValueNames())
            {
                var command = run.GetValue(name)?.ToString() ?? "";
                if (!SeatProcessClassifier.IsSecurityStartup(name) && !SeatProcessClassifier.IsSecurityStartup(command))
                    entries[target + "|" + name] = command;
            }
        }
        var common = Environment.GetFolderPath(Environment.SpecialFolder.CommonStartup);
        if (Directory.Exists(common))
            foreach (var file in Directory.EnumerateFiles(common))
            {
                var name = Path.GetFileName(file);
                if (name.Equals("desktop.ini", StringComparison.OrdinalIgnoreCase)) continue;
                var target = file;
                if (name.EndsWith(".lnk", StringComparison.OrdinalIgnoreCase))
                    try { target = new MultiSeat.Service.SteamShim.WScriptShortcutStore().Read(file)?.Target is { Length: > 0 } t ? t : file; }
                    catch (Exception) { /* unreadable shortcut: blocked by name only, never guessed */ }
                if (!SeatProcessClassifier.IsSecurityStartup(name) && !SeatProcessClassifier.IsSecurityStartup(target))
                    entries["StartupFolder|" + name] = target;
            }
        return entries;
    }

    // Shared hosts: a Run entry through them must not make every instance in the session a target.
    private static readonly HashSet<string> GenericHosts = new(StringComparer.OrdinalIgnoreCase)
    {
        "rundll32.exe", "cmd.exe", "powershell.exe", "pwsh.exe", "wscript.exe", "cscript.exe",
        "conhost.exe", "explorer.exe", "msiexec.exe", "mshta.exe", "regsvr32.exe", "svchost.exe"
    };

    /// <summary>File name of the executable a Run command starts ("C:\Program Files\X\x.exe" -a → x.exe).</summary>
    internal static string? ExecutableOf(string command)
    {
        var text = Environment.ExpandEnvironmentVariables(command.Trim());
        string path;
        if (text.StartsWith('"'))
        {
            var end = text.IndexOf('"', 1);
            path = end > 1 ? text[1..end] : text.Trim('"');
        }
        else
        {
            var exe = text.IndexOf(".exe", StringComparison.OrdinalIgnoreCase);
            path = exe > 0 ? text[..(exe + 4)] : text.Split(' ', StringSplitOptions.RemoveEmptyEntries).FirstOrDefault() ?? "";
        }
        var name = Path.GetFileName(path);
        return name.EndsWith(".exe", StringComparison.OrdinalIgnoreCase) ? name : null;
    }

    /// <summary>The seat account's own Run entries (OneDrive, launchers…), from its hive only. Kinseat's own
    /// entries (Steam shim of the seat) and security tools are never listed.</summary>
    internal static IReadOnlyDictionary<string, string> SeatRunEntries(RegistryKey seatRoot)
    {
        var entries = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        using var run = seatRoot.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Run");
        if (run is null) return entries;
        foreach (var name in run.GetValueNames())
        {
            var command = run.GetValue(name)?.ToString() ?? "";
            if (name.Equals("Steam", StringComparison.OrdinalIgnoreCase) ||
                command.Contains("kinseat", StringComparison.OrdinalIgnoreCase) ||
                command.Contains("multiseat", StringComparison.OrdinalIgnoreCase) ||
                SeatProcessClassifier.IsSecurityStartup(name) || SeatProcessClassifier.IsSecurityStartup(command)) continue;
            entries["Run|" + name] = command;
        }
        return entries;
    }

    /// <summary>Widgets (WidgetBoard, Edge WebView feeds) off for the seat account only.</summary>
    internal static void ApplyWidgets(RegistryKey seatRoot, bool block)
    {
        try
        {
            using var advanced = seatRoot.CreateSubKey(@"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced", true)!;
            if (block) advanced.SetValue("TaskbarDa", 0, RegistryValueKind.DWord);
            else advanced.DeleteValue("TaskbarDa", false);
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or IOException or System.Security.SecurityException)
        {
            // Some Windows builds protect this value; the sweep still applies.
        }
    }

    // Seat accounts must not be able to shut down or restart the host PC. This policy hides
    // Shut down / Restart / Sleep / Hibernate from Start, Ctrl+Alt+Del and Alt+F4 for this account only.
    // Always applied, independent of the lightening settings; only ever written to the hive of a seat
    // account created by Kinseat, never to the host console.
    internal static void ApplyPowerPolicy(RegistryKey root)
    {
        using var policy = root.CreateSubKey(Explorer, true)!;
        policy.SetValue("NoClose", 1, RegistryValueKind.DWord);
    }

    internal static readonly string[] DesktopIcons =
    [
        "{645FF040-5081-101B-9F08-00AA002F954E}", // Corbeille
        "{20D04FE0-3AEA-1069-A2D8-08002B30309D}", // Ce PC
        "{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}", // Réseau
        "{59031A47-3F72-44A7-89C5-5595FE6B30EE}", // Fichiers de l'utilisateur
        "{5399E694-6CE5-4D6C-8FCE-1D8870FDCBA0}", // Panneau de configuration
        "{2cc5ca98-6485-489a-920e-b3e88a6ccce3}"  // En savoir plus sur cette image
    ];
    internal static void ApplyDesktopIcons(RegistryKey root, bool empty)
    {
        // NoDesktop also hides game shortcuts. Undo our former policy even if allégement is off.
        using (var policy = root.CreateSubKey(Explorer, true)!) policy.DeleteValue("NoDesktop", false);
        foreach (var panel in new[] { "NewStartPanel", "ClassicStartMenu" })
        {
            using var icons = root.CreateSubKey(
                @"Software\Microsoft\Windows\CurrentVersion\Explorer\HideDesktopIcons\" + panel, true)!;
            foreach (var icon in DesktopIcons)
                if (empty) icons.SetValue(icon, 1, RegistryValueKind.DWord);
                else icons.DeleteValue(icon, false);
        }
    }

    internal static void ApplyApproved(RegistryKey root, IReadOnlyDictionary<string, string> entries, bool block)
    {
        foreach (var section in new[] { "Run", "Run32", "StartupFolder" })
        {
            using var approved = root.CreateSubKey(Approved + "\\" + section, true)!;
            using var owned = root.CreateSubKey(Owned + "\\" + section, true)!;
            if (block)
            {
                foreach (var pair in entries.Where(e => e.Key.StartsWith(section + "|", StringComparison.OrdinalIgnoreCase)))
                {
                    var name = pair.Key[(section.Length + 1)..];
                    if (SeatProcessClassifier.IsSecurityStartup(name) || SeatProcessClassifier.IsSecurityStartup(pair.Value)) continue;
                    var current = approved.GetValue(name) as byte[];
                    if (current is { Length: > 0 } && current[0] == 3) continue; // not ours
                    approved.SetValue(name, Disabled, RegistryValueKind.Binary);
                    owned.SetValue(name, pair.Value, RegistryValueKind.String);
                }
            }
            else
            {
                foreach (var name in owned.GetValueNames())
                {
                    var current = approved.GetValue(name) as byte[];
                    if (current is not null && current.SequenceEqual(Disabled))
                        approved.SetValue(name, Enabled, RegistryValueKind.Binary);
                    owned.DeleteValue(name, false);
                }
            }
        }
    }

    public void ApplyRegistry(string accountSid, SeatLightening settings)
    {
        if (!OperatingSystem.IsWindows()) return;
        // A SID is resolved from the account, never accepted from IPC. Refuse all non-user SIDs.
        var sid = new System.Security.Principal.SecurityIdentifier(accountSid);
        if (!accountSid.StartsWith("S-1-5-21-", StringComparison.Ordinal) ||
            !string.Equals(sid.Value, accountSid, StringComparison.Ordinal))
            throw new ArgumentException("SID de compte local invalide", nameof(accountSid));
        using var loaded = Registry.Users.OpenSubKey(accountSid, true);
        var mounted = loaded is null;
        string? mount = null;
        try
        {
            if (mounted)
            {
                using var profile = Registry.LocalMachine.OpenSubKey($@"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\{accountSid}");
                var path = profile?.GetValue("ProfileImagePath") as string
                    ?? throw new IOException("Profil Windows introuvable");
                var hive = Path.Combine(Environment.ExpandEnvironmentVariables(path), "NTUSER.DAT");
                if (!File.Exists(hive)) throw new FileNotFoundException("Ruche du compte introuvable", hive);
                EnableHivePrivileges();
                mount = "Kinseat-Lightening-" + accountSid;
                var result = RegLoadKey(HkeyUsers, mount, hive);
                if (result != 0) throw new Win32Exception(result, "RegLoadKey a échoué");
            }
            using var root = mounted ? Registry.Users.OpenSubKey(mount!, true) : Registry.Users.OpenSubKey(accountSid, true);
            if (root is null) throw new IOException("La ruche du compte ne peut être ouverte");
            ApplyDesktopIcons(root, settings.EmptyDesktop);
            ApplyPowerPolicy(root);
            var entries = new Dictionary<string, string>(StartupEntries(), StringComparer.OrdinalIgnoreCase);
            foreach (var (key, value) in SeatRunEntries(root)) entries.TryAdd(key, value);
            ApplyApproved(root, entries, settings.BlockStartupApps);
            ApplyWidgets(root, settings.BlockStartupApps);
            lock (_gate)
            {
                var startup = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
                foreach (var (key, value) in entries)
                    if (key.StartsWith("StartupFolder|", StringComparison.OrdinalIgnoreCase))
                    {
                        // A .lnk filename is not the executable it targets: never guess.
                        if (value.EndsWith(".exe", StringComparison.OrdinalIgnoreCase))
                            startup.Add(Path.GetFileName(value));
                    }
                    else if (ExecutableOf(value) is { } exe && !GenericHosts.Contains(exe))
                        startup.Add(exe);
                // Widgets: started by Windows for every session; Explorer's TaskbarDa is protected on some builds.
                startup.Add("WidgetBoard.exe");
                startup.Add("Widgets.exe");
                _startupBySid[accountSid] = startup;
                _blockedBySid[accountSid] = settings.BlockStartupApps ? entries.Keys.Order(StringComparer.OrdinalIgnoreCase).ToArray() : [];
            }
        }
        finally
        {
            if (mount is not null)
            {
                var result = RegUnLoadKey(HkeyUsers, mount);
                if (result != 0) logger.LogWarning("RegUnLoadKey {Mount} a échoué ({Code})", mount, result);
            }
        }
    }

    public object Report(Guid seatId, string accountName, SeatLightening settings)
    {
        var sid = AccountManager.TryResolveSid(accountName, logger);
        lock (_gate)
        {
            _stopped.TryGetValue(seatId, out var recent);
            return new { settings, stoppedProcesses = recent?.ToArray() ?? [],
                blockedStartupEntries = sid is not null && _blockedBySid.TryGetValue(sid, out var names)
                    ? names.ToArray() : [] };
        }
    }

    internal static IEnumerable<(ProcessSnapshot Process, string Category)> SelectProcesses(
        IEnumerable<ProcessSnapshot> processes, int sessionId, SeatLightening settings,
        IReadOnlySet<string> startupNames, int apolloPid, string? launchApp)
    {
        var all = processes.ToArray();
        var byPid = all.Where(p => p.SessionId == sessionId).ToDictionary(p => p.Pid);
        bool GameLineage(ProcessSnapshot p)
        {
            var visited = new HashSet<int>();
            var current = p;
            while (visited.Add(current.Pid))
            {
                if (current.Pid == apolloPid && apolloPid > 0 ||
                    (!string.IsNullOrWhiteSpace(launchApp) &&
                     string.Equals(current.Name, Path.GetFileName(launchApp), StringComparison.OrdinalIgnoreCase))) return true;
                if (!byPid.TryGetValue(current.ParentPid, out var parent)) break;
                current = parent;
            }
            return false;
        }
        foreach (var process in all.Where(p => p.SessionId == sessionId))
        {
            var startup = settings.BlockStartupApps && startupNames.Contains(process.Name);
            var category = SeatProcessClassifier.Classify(process.Name, process.SessionId, sessionId,
                startup, settings.BlockVendorUtilities, settings.CloseWindhawkTray, GameLineage(process));
            if (category is not null) yield return (process, category);
        }
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        if (!OperatingSystem.IsWindows()) return;
        var seats = services.GetRequiredService<SeatManager>();
        while (!stoppingToken.IsCancellationRequested)
        {
            try
            {
                foreach (var seat in seats.GetAllSeats())
                {
                    if (seat.SessionId < 1 || seat.Status is not (SeatStatus.Ready or SeatStatus.Streaming))
                    { lock (_gate) { _windows.Remove(seat.Id); _vendorStops.Remove(seat.Id); _vendorLimitLogged.RemoveWhere(k => k.Seat == seat.Id); } continue; }
                    DateTimeOffset started;
                    lock (_gate)
                    {
                        if (!_windows.TryGetValue(seat.Id, out var window) || window.Session != seat.SessionId)
                            _windows[seat.Id] = window = (seat.SessionId, DateTimeOffset.UtcNow);
                        started = window.Since;
                    }
                    var interval = DateTimeOffset.UtcNow - started < TimeSpan.FromMinutes(3) ? 5 : 30;
                    if ((DateTimeOffset.UtcNow - started).TotalSeconds % interval >= 5) continue;
                    Sweep(seat);
                }
            }
            catch (Exception ex) { logger.LogWarning(ex, "Balayage des sessions allégées impossible"); }
            await Task.Delay(TimeSpan.FromSeconds(5), stoppingToken);
        }
    }

    private void Sweep(SeatInfo seat)
    {
        var snapshots = new List<ProcessSnapshot>();
        using (var search = new ManagementObjectSearcher("SELECT ProcessId, ParentProcessId, SessionId, Name FROM Win32_Process"))
        using (var results = search.Get())
            foreach (ManagementObject row in results)
            {
                using (row)
                    if (row["ProcessId"] is uint pid && row["SessionId"] is uint session)
                        snapshots.Add(new((int)pid, (int)session, row["Name"] as string ?? "", Convert.ToInt32(row["ParentProcessId"] ?? 0)));
            }
        var sid = AccountManager.TryResolveSid(seat.AccountName, logger);
        HashSet<string> startup;
        lock (_gate) startup = sid is not null && _startupBySid.TryGetValue(sid, out var names)
            ? new HashSet<string>(names, StringComparer.OrdinalIgnoreCase) : new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var (candidate, category) in SelectProcesses(snapshots, seat.SessionId, seat.Lightening, startup,
                     seat.ApolloProcessId, seat.LaunchApp))
            try
            {
                if (category == "vendor")
                {
                    lock (_gate)
                    {
                        if (!_vendorStops.TryGetValue(seat.Id, out var counts))
                            _vendorStops[seat.Id] = counts = new(StringComparer.OrdinalIgnoreCase);
                        if (!MayStopVendor(counts.GetValueOrDefault(candidate.Name)))
                        {
                            if (_vendorLimitLogged.Add((seat.Id, candidate.Name.ToUpperInvariant())))
                                logger.LogWarning("Place {SeatId} : {Name} redémarre ; arrêt des tentatives après trois arrêts",
                                    seat.Id, candidate.Name);
                            continue;
                        }
                    }
                }
                using var process = Process.GetProcessById(candidate.Pid);
                if (process.SessionId != seat.SessionId || !process.ProcessName.Equals(
                        Path.GetFileNameWithoutExtension(candidate.Name), StringComparison.OrdinalIgnoreCase)) continue;
                process.Kill();
                logger.LogInformation("Place {SeatId} : arrêt {Name} PID {Pid}, catégorie {Category}",
                    seat.Id, candidate.Name, candidate.Pid, category);
                lock (_gate)
                {
                    if (!_stopped.TryGetValue(seat.Id, out var list)) _stopped[seat.Id] = list = [];
                    if (category == "vendor")
                    {
                        if (!_vendorStops.TryGetValue(seat.Id, out var counts))
                            _vendorStops[seat.Id] = counts = new(StringComparer.OrdinalIgnoreCase);
                        counts[candidate.Name] = counts.GetValueOrDefault(candidate.Name) + 1;
                    }
                    list.Add(new(candidate.Name, candidate.Pid, category, DateTimeOffset.UtcNow));
                    if (list.Count > 50) list.RemoveAt(0);
                }
            }
            catch (Exception ex) { logger.LogWarning(ex, "Arrêt de {Name} PID {Pid} impossible", candidate.Name, candidate.Pid); }
    }

    private static readonly nint HkeyUsers = new(unchecked((int)0x80000003));
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern int RegLoadKey(nint root, string subKey, string file);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern int RegUnLoadKey(nint root, string subKey);

    private static void EnableHivePrivileges()
    {
        if (!AdvApi.OpenProcessToken(Kernel32.GetCurrentProcess(),
                AdvApi.TOKEN_ADJUST_PRIVILEGES | AdvApi.TOKEN_QUERY, out var token))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        try
        {
            foreach (var privilege in new[] { "SeBackupPrivilege", "SeRestorePrivilege" })
            {
                if (!AdvApi.LookupPrivilegeValueW(null, privilege, out var luid))
                    throw new Win32Exception(Marshal.GetLastWin32Error());
                var state = new AdvApi.TOKEN_PRIVILEGES { PrivilegeCount = 1,
                    Privileges = new AdvApi.LUID_AND_ATTRIBUTES { Luid = luid, Attributes = AdvApi.SE_PRIVILEGE_ENABLED } };
                if (!AdvApi.AdjustTokenPrivileges(token, false, ref state, 0, IntPtr.Zero, IntPtr.Zero) ||
                    Marshal.GetLastWin32Error() == 1300)
                    throw new Win32Exception(Marshal.GetLastWin32Error(), privilege);
            }
        }
        finally { Kernel32.CloseHandle(token); }
    }
}
