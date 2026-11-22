using System.Diagnostics;
using System.Net;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

namespace MultiSeat.Service.Streaming;

/// <summary>
/// Submits a Moonlight client PIN to the loopback web UI of one seat's Apollo instance.
///
/// Apollo (ClassicOldSong/Apollo, src/confighttp.cpp) authenticates its web API with a session
/// cookie obtained from POST /api/login {username,password}; POST /api/pin {pin,name} then returns
/// {"status":true|false}. Apollo stores only a salted hash of the password, so Kinseat owns a
/// dedicated web account: it generates the secret, keeps it DPAPI-protected (LocalMachine) in a
/// SYSTEM/Administrators-only file, and installs it with Apollo's official
/// <c>sunshine.exe &lt;conf&gt; --creds &lt;user&gt; &lt;password&gt;</c> command.
/// </summary>
internal static class ApolloPairingClient
{
    internal const string KinseatWebUser = "kinseat";

    internal static bool Valid(string? pin, string? name) =>
        pin is { Length: 4 } && pin.All(c => c is >= '0' and <= '9') &&
        name is { Length: >= 1 and <= 40 } && !string.IsNullOrWhiteSpace(name) &&
        !name.Any(char.IsControl);

    internal static HttpClient CreateLoopbackClient() => new(new HttpClientHandler
    {
        UseProxy = false,
        AllowAutoRedirect = false,
        CookieContainer = new CookieContainer(),
        UseCookies = true,
        // Apollo generates its own self-signed certificate; only loopback is ever contacted.
        ServerCertificateCustomValidationCallback = (request, _, _, _) =>
            request.RequestUri is { Scheme: "https", Host: "127.0.0.1" },
    }) { Timeout = TimeSpan.FromSeconds(8) };

    internal enum Outcome { Paired, PinRejected, LoginRejected }

    /// <summary>Retry only PIN rejections: on first pairing the client may not yet have opened /pair.</summary>
    internal static async Task<Outcome> SubmitWithRetryAsync(
        Func<CancellationToken, Task<Outcome>> submit, CancellationToken ct,
        TimeSpan? interval = null, TimeSpan? maxWait = null)
    {
        var delay = interval ?? TimeSpan.FromSeconds(1);
        var deadline = Stopwatch.GetTimestamp() + (long)((maxWait ?? TimeSpan.FromSeconds(15)).TotalSeconds * Stopwatch.Frequency);
        while (true)
        {
            var outcome = await submit(ct);
            if (outcome != Outcome.PinRejected || Stopwatch.GetTimestamp() >= deadline)
                return outcome;
            var remaining = TimeSpan.FromSeconds((double)(deadline - Stopwatch.GetTimestamp()) / Stopwatch.Frequency);
            if (remaining <= TimeSpan.Zero) return Outcome.PinRejected;
            await Task.Delay(remaining < delay ? remaining : delay, ct);
        }
    }

    /// <summary>Logs in, then submits the PIN. A wrong or expired PIN returns PinRejected.</summary>
    internal static async Task<Outcome> SubmitAsync(HttpClient client, int webPort, string pin, string name,
        string username, string password, CancellationToken ct)
    {
        if (!Valid(pin, name)) throw new ArgumentException("PIN ou nom d'appareil invalide");
        if (webPort is < 1 or > 65535) throw new ArgumentOutOfRangeException(nameof(webPort));
        var root = $"https://127.0.0.1:{webPort}";
        try
        {
            using (var login = await client.PostAsync($"{root}/api/login", Json(new { username, password }), ct))
            {
                if (login.StatusCode is HttpStatusCode.Unauthorized or HttpStatusCode.Forbidden
                    or HttpStatusCode.Redirect or HttpStatusCode.Found or HttpStatusCode.TemporaryRedirect)
                    return Outcome.LoginRejected;
                if (!login.IsSuccessStatusCode)
                    throw new InvalidOperationException($"Connexion à Apollo refusée (HTTP {(int)login.StatusCode})");
            }
            using var response = await client.PostAsync($"{root}/api/pin", Json(new { pin, name }), ct);
            if (response.StatusCode is HttpStatusCode.Unauthorized or HttpStatusCode.Forbidden
                or HttpStatusCode.Redirect or HttpStatusCode.Found or HttpStatusCode.TemporaryRedirect)
                return Outcome.LoginRejected;
            if (!response.IsSuccessStatusCode)
                throw new InvalidOperationException($"Apollo a répondu HTTP {(int)response.StatusCode} au PIN");
            var body = await response.Content.ReadAsStringAsync(ct);
            using var document = JsonDocument.Parse(string.IsNullOrWhiteSpace(body) ? "{}" : body);
            return document.RootElement.ValueKind == JsonValueKind.Object
                && document.RootElement.TryGetProperty("status", out var status)
                && status.ValueKind == JsonValueKind.True
                ? Outcome.Paired
                : Outcome.PinRejected;
        }
        catch (HttpRequestException ex)
        {
            throw new InvalidOperationException("Interface web Apollo locale inaccessible", ex);
        }
        catch (JsonException)
        {
            return Outcome.PinRejected;
        }
    }

    // Apollo src/crypto.h, enum class PERM. A device paired through /api/pin only gets
    // PERM::_default = list | view: it can watch a stream someone else started, but it cannot
    // launch the seat's desktop nor send keyboard, mouse, controller, touch or pen input.
    internal const uint PermAllInputs = 0x1F00;          // controller | touch | pen | mouse | kbd
    internal const uint PermClipboard = 0x30000;         // clipboard_set | clipboard_read
    internal const uint PermAllActions = 0x7000000;      // list | view | launch
    /// <summary>What a family member's own device needs to play on its seat. File transfer,
    /// server commands and USB forwarding stay off (USB has its own opt-in in Kinseat).</summary>
    internal const uint PermPlay = PermAllInputs | PermClipboard | PermAllActions;

    /// <summary>
    /// Grants <see cref="PermPlay"/> to the paired device named <paramref name="name"/> (the most
    /// recent one when several share the name). POST /api/clients/update replaces every field, so
    /// the device's current values from /api/clients/list are sent back unchanged. Returns false
    /// when the device is not listed or Apollo refuses; the caller is already logged in.
    /// </summary>
    internal static async Task<bool> GrantPlayAsync(HttpClient client, int webPort, string name,
        CancellationToken ct)
    {
        var root = $"https://127.0.0.1:{webPort}";
        try
        {
            using var list = await client.GetAsync($"{root}/api/clients/list", ct);
            if (!list.IsSuccessStatusCode) return false;
            using var document = JsonDocument.Parse(await list.Content.ReadAsStringAsync(ct));
            if (!document.RootElement.TryGetProperty("named_certs", out var certs)
                || certs.ValueKind != JsonValueKind.Array) return false;
            JsonElement? device = null;
            foreach (var cert in certs.EnumerateArray())
                if (cert.ValueKind == JsonValueKind.Object
                    && cert.TryGetProperty("name", out var n) && n.ValueKind == JsonValueKind.String
                    && n.GetString() == name
                    && cert.TryGetProperty("uuid", out var u) && u.ValueKind == JsonValueKind.String)
                    device = cert;
            if (device is not { } d) return false;
            uint current = d.TryGetProperty("perm", out var p) && p.TryGetUInt32(out var v) ? v : 0;
            if ((current & PermPlay) == PermPlay) return true;
            var update = new Dictionary<string, object?>
            {
                ["uuid"] = d.GetProperty("uuid").GetString(),
                ["name"] = name,
                ["display_mode"] = Text(d, "display_mode"),
                ["perm"] = current | PermPlay,
                ["enable_legacy_ordering"] = Flag(d, "enable_legacy_ordering", true),
                ["allow_client_commands"] = Flag(d, "allow_client_commands", true),
                ["always_use_virtual_display"] = Flag(d, "always_use_virtual_display", false),
                ["do"] = d.TryGetProperty("do", out var doCmds) ? doCmds.Clone() : Array.Empty<object>(),
                ["undo"] = d.TryGetProperty("undo", out var undoCmds) ? undoCmds.Clone() : Array.Empty<object>(),
            };
            using var response = await client.PostAsync($"{root}/api/clients/update", Json(update), ct);
            if (!response.IsSuccessStatusCode) return false;
            using var result = JsonDocument.Parse(await response.Content.ReadAsStringAsync(ct) is { Length: > 0 } b ? b : "{}");
            return result.RootElement.ValueKind == JsonValueKind.Object
                && result.RootElement.TryGetProperty("status", out var status)
                && status.ValueKind == JsonValueKind.True;
        }
        catch (Exception ex) when (ex is HttpRequestException or JsonException or TaskCanceledException)
        {
            return false;
        }

        static string Text(JsonElement e, string key) =>
            e.TryGetProperty(key, out var x) && x.ValueKind == JsonValueKind.String ? x.GetString()! : "";
        static bool Flag(JsonElement e, string key, bool fallback) =>
            e.TryGetProperty(key, out var x) && x.ValueKind is JsonValueKind.True or JsonValueKind.False
                ? x.GetBoolean() : fallback;
    }

    private static StringContent Json(object value) =>
        new(JsonSerializer.Serialize(value), Encoding.UTF8, "application/json");

    // ── Kinseat-owned Apollo web credentials ─────────────────────────────

    internal static string SecretPath(string apolloConfigDir) =>
        Path.Combine(apolloConfigDir, "kinseat-web.secret");

    /// <summary>Returns the stored password, or null when Kinseat has not installed its account yet.</summary>
    internal static string? ReadSecret(string apolloConfigDir)
    {
        var path = SecretPath(apolloConfigDir);
        if (!OperatingSystem.IsWindows() || !File.Exists(path)) return null;
        try
        {
            var blob = Convert.FromBase64String(File.ReadAllText(path).Trim());
            return Encoding.UTF8.GetString(ProtectedData.Unprotect(blob, null, DataProtectionScope.LocalMachine));
        }
        catch (Exception) { return null; }
    }

    internal static string NewPassword() =>
        Convert.ToHexString(RandomNumberGenerator.GetBytes(24)).ToLowerInvariant();

    /// <summary>
    /// Installs a fresh Kinseat web account into the shared Apollo credentials file through Apollo's
    /// own <c>--creds</c> command, then stores the secret. Running Apollo instances read the
    /// credentials at start-up, so the caller restarts the seat's Apollo afterwards.
    /// </summary>
    internal static async Task<string> InstallAccountAsync(string apolloExe, string seatConfPath,
        string apolloConfigDir, CancellationToken ct)
    {
        if (!OperatingSystem.IsWindows()) throw new PlatformNotSupportedException();
        if (!File.Exists(apolloExe)) throw new InvalidOperationException("Apollo introuvable : " + apolloExe);
        var password = NewPassword();
        var start = new ProcessStartInfo(apolloExe)
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            WorkingDirectory = Path.GetDirectoryName(apolloExe)!,
        };
        start.ArgumentList.Add(seatConfPath);
        start.ArgumentList.Add("--creds");
        start.ArgumentList.Add(KinseatWebUser);
        start.ArgumentList.Add(password);
        using var process = Process.Start(start) ?? throw new InvalidOperationException("Apollo --creds n'a pas démarré");
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct);
        timeout.CancelAfter(TimeSpan.FromSeconds(20));
        var stdout = process.StandardOutput.ReadToEndAsync(timeout.Token);
        var stderr = process.StandardError.ReadToEndAsync(timeout.Token);
        try { await process.WaitForExitAsync(timeout.Token); }
        catch (OperationCanceledException)
        {
            try { process.Kill(true); } catch (InvalidOperationException) { }
            throw new InvalidOperationException("Apollo --creds ne répond pas");
        }
        if (process.ExitCode != 0)
            throw new InvalidOperationException($"Apollo --creds a échoué (code {process.ExitCode})");
        _ = await Task.WhenAll(stdout, stderr);
        var blob = ProtectedData.Protect(Encoding.UTF8.GetBytes(password), null, DataProtectionScope.LocalMachine);
        Directory.CreateDirectory(apolloConfigDir);
        var path = SecretPath(apolloConfigDir);
        File.WriteAllText(path, Convert.ToBase64String(blob));
        Storage.SecureFile.RestrictToSystemAndAdmins(path);
        return password;
    }
}
