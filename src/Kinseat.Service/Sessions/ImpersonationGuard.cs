using System.Security.Principal;
using Microsoft.Extensions.Logging;
using MultiSeat.Service.Interop;

namespace MultiSeat.Service.Sessions;

/// <summary>
/// The service's privileged code (WTSQueryUserToken, SeTcbPrivilege, LogonUser + TokenSessionId)
/// only works under the process identity (LocalSystem). A thread that still carries an
/// impersonation token, for instance the unelevated Kinseat Host app that called the admin pipe,
/// fails with 1314 / 5. This guard makes that state visible and removes it.
/// </summary>
internal static class ImpersonationGuard
{
    /// <summary>Name of the identity the calling thread impersonates, or null when it runs as the process.</summary>
    internal static string? CurrentImpersonatedIdentity()
    {
        try
        {
            // ifImpersonating: true returns null when the thread has no impersonation token.
            using var identity = WindowsIdentity.GetCurrent(true);
            return identity is null ? null : (identity.Name ?? identity.User?.Value ?? "?");
        }
        catch (Exception)
        {
            return null;
        }
    }

    /// <summary>
    /// Logs a warning when the thread is impersonating, then drops the impersonation so the
    /// rest of the call runs under the process identity. Returns true when it had to revert.
    /// </summary>
    internal static bool EnsureProcessIdentity(ILogger logger, string where)
    {
        var name = CurrentImpersonatedIdentity();
        if (name is null) return false;
        logger.LogWarning(
            "{Where} is running while impersonating {Identity}; reverting to the service identity. " +
            "A caller leaked its impersonation into privileged code.", where, name);
        if (!AdvApi.RevertToSelf())
            logger.LogWarning("RevertToSelf failed in {Where} (Win32 {Err})", where,
                System.Runtime.InteropServices.Marshal.GetLastWin32Error());
        return true;
    }
}
