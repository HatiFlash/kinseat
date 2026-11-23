using System.IO.Pipes;
using System.Security.AccessControl;
using System.Security.Principal;
using Microsoft.Win32.SafeHandles;
using System.Text;
using System.Text.Json;
using MultiSeat.Service.Accounts;
using MultiSeat.Service.Api;
using MultiSeat.Service.Configuration;
using MultiSeat.Service.Gpu;
using MultiSeat.Service.Monitoring;
using MultiSeat.Service.Sessions;
using MultiSeat.Service.Streaming;
using Microsoft.Extensions.Options;
using MultiSeat.Shared.Models;

namespace MultiSeat.Service.Ipc;

/// <summary>Local administrative pipe. Never use HTTP authentication as a substitute for its ACL.</summary>
public sealed class AdminPipeServer(
    SeatManager seats, MetricsCollector metrics, NoticeScheduler notices, PreviewBroker previews,
    ILogger<AdminPipeServer> logger, MultiSeat.Service.Logging.DiagnosticLevelStore diagnosticLevel,
    MultiSeat.Service.Storage.SeatDefinitionStore definitions, AccountManager accounts,
    SeatPresetStore presets, DwmFrameIntervalSetting refreshRate,
    AdminAuthorizationStore authorizations, JoinRequestStore joinRequests,
    GpuBudgetService gpuBudget, SeatLighteningService lightening,
    FirewallManager firewall, IOptions<MultiSeatOptions> options, SessionLauncher sessions,
    IEnumerable<IAdminMethodHandler> extraHandlers, MultiSeat.Service.Library.LibraryStore library,
    MultiSeat.Service.Library.GameShortcuts? gameShortcuts = null,
    MultiSeat.Service.Library.GameLaunchManifestWriter? gameManifests = null) : BackgroundService
{
    internal const string PipeName = "Kinseat.Admin.v1";
    internal const int MaxLineBytes = 65_536;
    internal const int MaxResponseBytes = 4 * 1024 * 1024;
    private const int MaxInstances = 4;
    /// <summary>Must stay above <see cref="Sessions.SeatManager.PairDeviceWait"/> plus the Apollo calls.</summary>
    internal static readonly TimeSpan PairRequestTimeout = TimeSpan.FromSeconds(45);
    internal static readonly TimeSpan DefaultRequestTimeout = TimeSpan.FromSeconds(10);
    internal static readonly TimeSpan DiagnosticsExportTimeout = TimeSpan.FromSeconds(60);
    /// <summary>Session provisioning and similar (seat.create ~36 s) must still be able to answer.</summary>
    internal static readonly TimeSpan LongRequestTimeout = TimeSpan.FromMinutes(5);
    internal static readonly TimeSpan AccountDeleteTimeout = TimeSpan.FromSeconds(120);
    private static readonly string[] LongMethods =
    {
        "seat.create", "seat.delete", "seat.resolution.set", "seat.preset.set",
        "seat.repair", "seat.action", "joinRequest.accept", "seat.graphics.set"
    };

    /// <summary>Request time budget from the raw request line; matches the quoted method value only.</summary>
    internal static TimeSpan RequestTimeoutFor(string rawLine)
    {
        if (rawLine.Contains("\"diagnostics.export\"", StringComparison.Ordinal)) return DiagnosticsExportTimeout;
        if (rawLine.Contains("\"account.delete\"", StringComparison.Ordinal)) return AccountDeleteTimeout;
        // seat.pair waits up to 20 s for the device to check the code; a wrong code must come
        // back as "Code refusé", not as a pipe cut at 10 s.
        if (rawLine.Contains("\"seat.pair\"", StringComparison.Ordinal)) return PairRequestTimeout;
        foreach (var method in LongMethods)
            if (rawLine.Contains("\"" + method + "\"", StringComparison.Ordinal)) return LongRequestTimeout;
        return DefaultRequestTimeout;
    }

    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web)
    {
        Converters = { new System.Text.Json.Serialization.JsonStringEnumConverter() }
    };

    private PipeSecurity CreateSecurity() => CreateSecurity(authorizations.GetAll().Select(a => a.Sid));

    /// <summary>
    /// SYSTEM et Administrateurs (élevés) en contrôle total ; chaque compte autorisé une fois
    /// obtient lecture/écriture pour son jeton non élevé. Le réseau reste refusé.
    /// </summary>
    internal static PipeSecurity CreateSecurity(IEnumerable<string> authorizedSids)
    {
        var security = new PipeSecurity();
        security.SetAccessRuleProtection(isProtected: true, preserveInheritance: false);
        // A remote administrator must not turn this into an SMB-accessible admin API.
        security.AddAccessRule(new PipeAccessRule(
            new SecurityIdentifier(WellKnownSidType.NetworkSid, null),
            PipeAccessRights.FullControl, AccessControlType.Deny));
        foreach (var sid in new[] { WellKnownSidType.LocalSystemSid, WellKnownSidType.BuiltinAdministratorsSid })
            security.AddAccessRule(new PipeAccessRule(
                new SecurityIdentifier(sid, null), PipeAccessRights.FullControl, AccessControlType.Allow));
        foreach (var sid in authorizedSids.Where(AdminAuthorizationStore.IsValidSid).Distinct(StringComparer.OrdinalIgnoreCase))
            security.AddAccessRule(new PipeAccessRule(
                new SecurityIdentifier(sid), PipeAccessRights.ReadWrite, AccessControlType.Allow));
        return security;
    }

    /// <summary>
    /// Applique la DACL courante. Les instances d'un même canal partagent leur descripteur :
    /// le mettre à jour sur une instance vaut pour celle qui attend la prochaine connexion.
    /// </summary>
    private void RefreshSecurity(NamedPipeServerStream pipe)
    {
        try { pipe.SetAccessControl(CreateSecurity()); }
        catch (Exception ex) when (ex is UnauthorizedAccessException or IOException or InvalidOperationException)
        {
            logger.LogWarning(ex, "Kinseat admin pipe security refresh failed");
        }
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        if (!OperatingSystem.IsWindows()) return;
        var clients = new List<Task>();
        while (!stoppingToken.IsCancellationRequested)
        {
            NamedPipeServerStream? pipe = null;
            try
            {
                // Explicit DACL; never use CurrentUserOnly (service SYSTEM != console admin).
                pipe = NamedPipeServerStreamAcl.Create(PipeName, PipeDirection.InOut, MaxInstances,
                    PipeTransmissionMode.Byte, PipeOptions.Asynchronous, 0, 0, CreateSecurity(),
                    HandleInheritability.None, PipeAccessRights.ChangePermissions);
                RefreshSecurity(pipe);
                await pipe.WaitForConnectionAsync(stoppingToken);
                var connected = pipe;
                pipe = null;
                clients.RemoveAll(t => t.IsCompleted);
                clients.Add(HandleClientAsync(connected, stoppingToken));
            }
            catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested) { break; }
            catch (Exception ex)
            {
                logger.LogError(ex, "Kinseat admin pipe accept failed");
                try { await Task.Delay(1000, stoppingToken); }
                catch (OperationCanceledException) { break; }
            }
            finally { pipe?.Dispose(); }
        }
        await Task.WhenAll(clients);
    }

    // A named profile without budget keeps the seat's budget only when the profile is unchanged;
    // switching profile (e.g. Economy -> Quality) takes the new profile's default budget.
    internal static GraphicsSelection ForExistingSeat(JsonElement p, SeatInfo seat) =>
        p.TryGetProperty("graphicsProfile", out _) ?
            (p.TryGetProperty("vramBudgetMb", out var requested) && requested.ValueKind != JsonValueKind.Null
                ? GraphicsSelection.Parse(p)
                : GraphicsSelection.Parse(p) is var parsed && parsed.Profile == seat.GraphicsProfile
                    ? parsed with { BudgetMb = seat.VramBudgetMb } : parsed) :
        new GraphicsSelection(seat.GraphicsProfile, seat.Width, seat.Height, seat.Fps,
            seat.NvencPreset, p.TryGetProperty("vramBudgetMb", out var budget) && budget.ValueKind != JsonValueKind.Null
                ? budget.GetInt32() : seat.VramBudgetMb);

    private async Task<SeatInfo> ApplyGraphicsAsync(SeatInfo seat, GraphicsSelection graphics, CancellationToken ct)
    {
        using var mutation = await gpuBudget.AcquireMutationAsync(ct);
        gpuBudget.RequireFits(seats.GetAllSeats(), graphics.BudgetMb, seat.Id);
        if (seat.Width != graphics.Width || seat.Height != graphics.Height)
            await seats.SetResolutionAsync(seat.Id, graphics.Width, graphics.Height, presets, ct);
        // FPS has no independent setter: Apollo reads it when restarted by SetNvencPresetAsync.
        var fpsChanged = seat.Fps != graphics.Fps;
        seat.Fps = graphics.Fps;
        if (fpsChanged || seat.NvencPreset != graphics.Nvenc)
            await seats.SetNvencPresetAsync(seat.Id, graphics.Nvenc, presets, ct);
        seat.GraphicsProfile = graphics.Profile;
        seat.VramBudgetMb = graphics.BudgetMb;
        if (seat.AutoStart) presets.Upsert(SeatPreset.ForAutoStart(seat));
        seat.VramUsedMb = gpuBudget.Usage(seat.SessionId);
        return seat;
    }

    internal static AdminFault AccountDeleteFault(string reason) =>
        new(reason == "account_unknown" ? "not_found" : "forbidden", AccountDeleteMessage(reason));

    internal static string AccountDeleteMessage(string reason) => reason switch
    {
        "account_unknown" => "Compte introuvable",
        "not_managed" => "Ce compte n'a pas été créé par Kinseat et ne peut pas être supprimé ici.",
        "caller" => "Impossible de supprimer le compte de l'appelant",
        "admin" => "Les comptes administrateurs ne peuvent pas être supprimés.",
        "console" => "Impossible de supprimer le compte de la session console",
        _ => "Identité du compte non vérifiable : suppression refusée"
    };

    private async Task<object> DeleteSeatAndAccountAsync(SeatInfo? target, string accountName, string? callerSid,
        bool purge, CancellationToken ct)
    {
        var steps = new SeatPurgeSteps();
        string? accountKept = null;
        if (purge)
        {
            try { accountKept = accounts.PurgeEligibility(accountName, callerSid); }
            catch (Exception ex) { accountKept = "identity_unknown"; steps.Failed.Add(new("account", ex.Message)); }
        }
        if (target is not null)
        {
            // Pairings are still accessible before teardown.
            await steps.Run("pairings", () => { seats.UnpairAllClients(target.Id); return Task.CompletedTask; });
            await SeatEndpoints.DeleteSeatAsync(target.Id, seats, ct);
            await steps.Run("preset", () => { presets.DeleteByAccount(accountName); return Task.CompletedTask; });
            await steps.Run("firewall", () => firewall.ClosePortsAsync(target, ct));
        }
        else await steps.Run("preset", () => { presets.DeleteByAccount(accountName); return Task.CompletedTask; });
        await steps.Run("publicDesktop", () => { seats.RemovePublicDesktopDeny(accountName); return Task.CompletedTask; });
        await steps.Run("shortcuts", () => { gameShortcuts?.Clear(accountName); return Task.CompletedTask; });
        await steps.Run("gameManifests", () => { gameManifests?.Clear(accountName); return Task.CompletedTask; });
        await steps.Run("library", () => { library.RevokeAccount(accountName); return Task.CompletedTask; });
        if (purge && accountKept is null)
        {
            await steps.Run("session", () =>
            {
                var sessionId = target?.SessionId > 0 ? target.SessionId : sessions.FindExistingSession(accountName);
                if (sessionId > 0)
                {
                    var verdict = sessions.CheckSession(sessionId, accountName).Verdict;
                    if (verdict is SessionLauncher.SessionVerdict.Active or SessionLauncher.SessionVerdict.Disconnected)
                        sessions.LogoffSession(sessionId);
                    if (sessions.CheckSession(sessionId, accountName).Verdict != SessionLauncher.SessionVerdict.Gone)
                        throw new IOException("Session du compte encore présente ou réattribuée");
                }
                return Task.CompletedTask;
            });
            await steps.Run("apollo", () =>
            {
                var root = Path.GetFullPath(options.Value.ApolloConfigDir);
                var dir = Path.GetFullPath(Path.Combine(root, accountName));
                if (!dir.StartsWith(Path.TrimEndingDirectorySeparator(root) + Path.DirectorySeparatorChar,
                        StringComparison.OrdinalIgnoreCase))
                    throw new IOException("Dossier Apollo hors du répertoire configuré");
                if (Directory.Exists(dir))
                {
                    if ((File.GetAttributes(dir) & FileAttributes.ReparsePoint) != 0)
                        throw new IOException("Dossier Apollo redirigé");
                    Directory.Delete(dir, recursive: true);
                }
                return Task.CompletedTask;
            });
            if (!steps.Failed.Any(f => f.item is "session" or "apollo"))
            {
                try { accountKept = accounts.PurgeEligibility(accountName, callerSid); }
                catch (Exception ex) { accountKept = "identity_unknown"; steps.Failed.Add(new("account", ex.Message)); }
                if (accountKept is null)
                {
                    await steps.Run("profile", () => { accounts.PurgeProfile(accountName); return Task.CompletedTask; });
                    if (!steps.Failed.Any(f => f.item == "profile"))
                    {
                        await steps.Run("account", () => { accounts.PurgeLocalAccount(accountName); return Task.CompletedTask; });
                        if (!steps.Failed.Any(f => f.item == "account"))
                            await steps.Run("registry", () => { accounts.ForgetPurgedAccount(accountName); return Task.CompletedTask; });
                    }
                }
            }
        }
        return new { ok = true, removed = steps.Removed, failed = steps.Failed, accountKept };
    }

    private async Task HandleClientAsync(NamedPipeServerStream pipe, CancellationToken ct)
    {
        await using (pipe)
        {
            try
            {
                // One request per connection; a slow peer cannot monopolise the accept loop.
                using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct);
                timeout.CancelAfter(DefaultRequestTimeout);
                // Windows only allows impersonating the peer once data has been read from the pipe,
                // so the bounded request line is read first, then the caller is identified before
                // anything is dispatched. No client-supplied user name or secret is accepted.
                var line = await ReadLineBoundedAsync(pipe, timeout.Token);
                if (line is not null)
                    timeout.CancelAfter(RequestTimeoutFor(Encoding.UTF8.GetString(line)));
                var name = "?";
                string? sid = null;
                var elevated = false;
                var isSystem = false;
                var member = false;
                pipe.RunAsClient(() =>
                {
                    using var identity = WindowsIdentity.GetCurrent();
                    name = identity.Name;
                    sid = identity.User?.Value;
                    // A UAC-filtered admin token carries Administrators as deny-only: IsInRole is false.
                    elevated = new WindowsPrincipal(identity).IsInRole(WindowsBuiltInRole.Administrator);
                    isSystem = identity.User?.IsWellKnown(WellKnownSidType.LocalSystemSid) == true;
                    member = AdminAuthorizationStore.IsAdministratorsMember(identity.Claims);
                });
                // RunAsClient reverts in a finally, but make a leaked impersonation impossible to miss.
                ImpersonationGuard.EnsureProcessIdentity(logger, "AdminPipeServer after RunAsClient");
                var listed = authorizations.Contains(sid);
                var authorized = AdminAuthorizationStore.IsAllowed(elevated, isSystem, listed, member);
                logger.LogInformation("Kinseat admin pipe client: {Identity}, elevated: {Elevated}, authorized account: {Listed}, allowed: {Allowed}",
                    name, elevated, listed, authorized);
                object reply;
                if (!authorized)
                    reply = AdminDispatcher.Error(null, "forbidden", listed
                        ? "Ce compte n'est plus administrateur de ce PC"
                        : "Accès réservé à un administrateur : autorisez Kinseat une fois en mode élevé");
                else
                    reply = line is null ? AdminDispatcher.Error(null, "request_too_large", "Requête absente ou supérieure à 64 Ko")
                    // Every privileged call below (WTSQueryUserToken, SeTcbPrivilege, LogonUser) needs the
                    // service's own identity. Run the whole dispatch explicitly as the process, so nothing
                    // can execute as the unelevated pipe client even if an impersonation leaked.
                    : await WindowsIdentity.RunImpersonatedAsync(SafeAccessTokenHandle.InvalidHandle,
                        () => AdminDispatcher.DispatchAsync(line!, async (method, parameters) =>
                    {
                        if (method == "app.authorization")
                            return new { authorized = listed, elevated };
                        if (method == "app.authorize")
                        {
                            var account = name.Split('\\').LastOrDefault() ?? name;
                            var seatAccounts = definitions.GetAll().Select(s => s.AccountName)
                                .Concat(accounts.ListManagedAccounts().Where(a => a.IsManaged).Select(a => a.Username));
                            var refusal = AdminAuthorizationStore.AuthorizeRefusal(elevated, sid, account, seatAccounts);
                            if (refusal is not null)
                            {
                                logger.LogWarning("Kinseat admin authorization refused for {Identity}: {Reason}", name, refusal);
                                throw new AdminFault("forbidden", refusal);
                            }
                            var added = authorizations.Add(sid!, name, DateTimeOffset.UtcNow);
                            RefreshSecurity(pipe);
                            logger.LogInformation("Kinseat admin authorization {State} for {Identity}", added ? "granted" : "already present", name);
                            return new { authorized = true, elevated };
                        }
                        if (method == "app.revoke")
                        {
                            var removed = sid is not null && authorizations.Remove(sid);
                            RefreshSecurity(pipe);
                            logger.LogInformation("Kinseat admin authorization {State} for {Identity}", removed ? "revoked" : "absent", name);
                            return new { authorized = false, elevated };
                        }
                        if (method == "diagnostics.getLevel") return new { level = diagnosticLevel.Level };
                        if (method == "diagnostics.setLevel")
                        {
                            var level = parameters.GetProperty("level").GetString()!;
                            diagnosticLevel.Set(level);
                            logger.LogInformation("Kinseat diagnostics level changed to {Level} by {Identity}", level, name);
                            return new { level = diagnosticLevel.Level };
                        }
                        if (method == "diagnostics.export")
                        {
                            var known = definitions.GetAll().Select(s => s.AccountName).ToArray();
                            var state = new { machine = DiagnosticExporter.MachineDetails(), health = metrics.Collect(seats),
                                seats = seats.GetAllSeats(), notices = notices.List(),
                                settings = new { level = diagnosticLevel.Level, preview = previews.Settings.ShowIndicator } };
                            var hostAccount = name.Split('\\').LastOrDefault();
                            var path = await Task.Run(() => DiagnosticExporter.Export(state, known, hostAccount: hostAccount), ct);
                            logger.LogInformation("Kinseat diagnostics exported by {Identity}", name);
                            return new { path };
                        }
                        if (method == "system.health") return metrics.Collect(seats);
                        if (method == "seats.list")
                        {
                            var all = seats.GetAllSeats();
                            foreach (var s in all) s.VramUsedMb = gpuBudget.Usage(s.SessionId);
                            return all;
                        }
                        if (method == "gpu.budget") return gpuBudget.Snapshot(seats.GetAllSeats());
                        if (method == "gpu.budget.setReserve")
                        {
                            using var mutation = await gpuBudget.AcquireMutationAsync(ct);
                            gpuBudget.SetReserve(parameters.GetProperty("hostReserveMb").GetInt32());
                            return gpuBudget.Snapshot(seats.GetAllSeats());
                        }
                        var handler = extraHandlers.FirstOrDefault(h => h.Methods.Contains(method, StringComparer.Ordinal));
                        if (handler is not null)
                            return await handler.HandleAsync(method, parameters, ct);
                        if (method == "accounts.list") return accounts.ListManagedAccounts().Select(a =>
                        {
                            string? reason;
                            try { reason = accounts.PurgeEligibility(a.Username, sid); }
                            catch (Exception) { reason = "identity_unknown"; }
                            return new
                            {
                                a.Username, a.IsManaged, managed = a.IsManaged, deletable = reason is null,
                                notDeletableReason = reason,
                                HasSeat = seats.GetAllSeats().Any(s => s.AccountName.Equals(a.Username, StringComparison.OrdinalIgnoreCase))
                            };
                        }).ToArray();
                        if (method == "account.delete")
                        {
                            var accountName = parameters.GetProperty("accountName").GetString()!;
                            string? reason;
                            try { reason = accounts.PurgeEligibility(accountName, sid); }
                            catch (Exception) { reason = "identity_unknown"; }
                            if (reason is not null) throw AccountDeleteFault(reason);
                            var target = seats.GetAllSeats().FirstOrDefault(s =>
                                s.AccountName.Equals(accountName, StringComparison.OrdinalIgnoreCase));
                            var result = await DeleteSeatAndAccountAsync(target, accountName, sid, purge: true, ct);
                            logger.LogInformation("Kinseat admin mutation {Method}, account {Account}, caller {Identity}", method, accountName, name);
                            return result;
                        }
                        if (method == "account.create")
                        {
                            var username = parameters.GetProperty("name").GetString()!;
                            ApiInputValidation.TryParseCredential(parameters, out var credential);
                            var account = AccountEndpoints.CreateValidated(
                                new AccountCreateRequest(username,
                                    parameters.TryGetProperty("password", out var legacy) ? legacy.GetString() : null,
                                    credential), accounts.CreateAccount);
                            logger.LogInformation("Kinseat admin mutation {Method}, account {Account}, caller {Identity}", method, username, name);
                            return account;
                        }
                        if (method == "seat.create")
                        {
                            var accountName = parameters.GetProperty("accountName").GetString()!;
                            if (!accounts.AccountExists(accountName))
                                throw new AdminFault("not_found", "Compte introuvable : créez un compte depuis l'application");
                            var displayName = parameters.TryGetProperty("displayName", out var display) ? display.GetString() : null;
                            var preset = parameters.TryGetProperty("preset", out var quality)
                                ? Enum.Parse<NvencQualityPreset>(quality.GetString()!, ignoreCase: false)
                                : NvencQualityPreset.Balanced;
                            var graphics = parameters.TryGetProperty("graphicsProfile", out _)
                                ? GraphicsSelection.Parse(parameters)
                                : new GraphicsSelection(GraphicsProfile.Balanced, 1920, 1080, 60,
                                    preset, parameters.TryGetProperty("vramBudgetMb", out var requestedBudget)
                                        ? requestedBudget.GetInt32() : 3072);
                            using var mutation = await gpuBudget.AcquireMutationAsync(ct);
                            gpuBudget.RequireFits(seats.GetAllSeats(), graphics.BudgetMb);
                            SeatInfo created;
                            try
                            {
                                created = await SeatEndpoints.ProvisionValidatedAsync(
                                    new SeatRequest { AccountName = accountName, DisplayName = displayName,
                                        Width = graphics.Width, Height = graphics.Height, Fps = graphics.Fps,
                                        NvencPreset = graphics.Nvenc, GraphicsProfile = graphics.Profile,
                                        VramBudgetMb = graphics.BudgetMb, AutoStart = true }, seats.ProvisionSeatAsync, presets, ct);
                            }
                            catch (Exception ex) when (ex is not OperationCanceledException)
                            {
                                logger.LogWarning(ex, "Kinseat admin seat.create failed for {Account}", accountName);
                                throw;
                            }
                            created.VramUsedMb = gpuBudget.Usage(created.SessionId);
                            logger.LogInformation("Kinseat admin mutation {Method}, seat {SeatId}, caller {Identity}", method, created.Id, name);
                            return created;
                        }
                        if (method == "host.refreshRate.get")
                            return SystemEndpoints.RefreshRateStatus(refreshRate, DwmFrameIntervalRegistry.Read());
                        if (method == "host.refreshRate.set")
                        {
                            var result = await SystemEndpoints.SetRefreshRateAsync(
                                new SystemEndpoints.RefreshRateRequest(parameters.GetProperty("intervalMs").GetInt32()),
                                refreshRate, ms => DwmFrameIntervalRegistry.Write(ms, refreshRate.Logger),
                                Path.Combine(AppContext.BaseDirectory, "appsettings.local.json"), logger);
                            logger.LogInformation("Kinseat admin mutation {Method}, caller {Identity}", method, name);
                            return AdminResults.Unwrap(result);
                        }
                        if (method == "joinRequests.list")
                            return joinRequests.List().Select(r => new
                            {
                                id = r.Id, pseudo = r.Pseudo, device = r.Device, address = r.Address,
                                createdAt = r.CreatedAt, status = JoinRequestListener.StatusName(r.Status)
                            }).ToArray();
                        if (method == "joinRequest.accept")
                        {
                            var requestId = Guid.Parse(parameters.GetProperty("requestId").GetString()!);
                            var targetId = Guid.Parse(parameters.GetProperty("seatId").GetString()!);
                            var target = seats.GetSeat(targetId) ?? throw new AdminFault("not_found", "Siège introuvable");
                            if (target.Status is not (SeatStatus.Ready or SeatStatus.Streaming))
                                throw new AdminFault("conflict", "Le siège n'est pas prêt");
                            // Validate the pending request before changing the seat; an expired request must not alter its profile.
                            if (joinRequests.List().All(r => r.Id != requestId || r.Status != JoinStatus.Pending))
                                throw new AdminFault("conflict", "Demande introuvable ou déjà traitée");
                            if (parameters.TryGetProperty("graphicsProfile", out _) ||
                                parameters.TryGetProperty("vramBudgetMb", out var requestedBudget) && requestedBudget.ValueKind != JsonValueKind.Null)
                                await ApplyGraphicsAsync(target, ForExistingSeat(parameters, target), ct);
                            var accepted = joinRequests.Accept(requestId, targetId);
                            if (accepted == JoinTransition.NotFound) throw new AdminFault("not_found", "Demande introuvable");
                            if (accepted == JoinTransition.WrongState) throw new AdminFault("conflict", "Demande déjà traitée ou expirée");
                            logger.LogInformation("Kinseat admin mutation {Method}, request {RequestId}, seat {SeatId}, caller {Identity}", method, requestId, targetId, name);
                            return new { id = requestId, status = "accepted" };
                        }
                        if (method == "joinRequest.refuse")
                        {
                            var requestId = Guid.Parse(parameters.GetProperty("requestId").GetString()!);
                            var refused = joinRequests.Refuse(requestId);
                            if (refused == JoinTransition.NotFound) throw new AdminFault("not_found", "Demande introuvable");
                            if (refused == JoinTransition.WrongState) throw new AdminFault("conflict", "Demande déjà traitée ou expirée");
                            logger.LogInformation("Kinseat admin mutation {Method}, request {RequestId}, caller {Identity}", method, requestId, name);
                            return new { id = requestId, status = "refused" };
                        }
                        if (method == "notices.list") return notices.List();
                        if (method == "settings.get") return new { preview = new { showIndicator = previews.Settings.ShowIndicator } };
                        if (method == "settings.set")
                        {
                            var enabled = parameters.GetProperty("preview").GetProperty("showIndicator").GetBoolean();
                            previews.Settings.SetShowIndicator(enabled);
                            logger.LogInformation("Kinseat admin mutation {Method}, caller {Identity}, preview.showIndicator {Enabled}", method, name, enabled);
                            return new { preview = new { showIndicator = enabled } };
                        }
                        if (method == "host.logonTasks.list") return await LogonTaskManager.ListAsync(ct);
                        if (method is "host.logonTasks.restrict" or "host.logonTasks.restore")
                        {
                            var path = parameters.GetProperty("path").GetString()!;
                            var task = parameters.GetProperty("name").GetString()!;
                            logger.LogInformation("Kinseat admin mutation {Method}, task {Path}{Task}, caller {Identity}",
                                method, path, task, name);
                            return method == "host.logonTasks.restrict"
                                ? await LogonTaskManager.RestrictAsync(path, task, ct)
                                : await LogonTaskManager.RestoreAsync(path, task, ct);
                        }
                        var seatId = Guid.Parse(parameters.GetProperty("seatId").GetString()!);
                        if (method == "seat.notice.cancel")
                        {
                            logger.LogInformation("Kinseat admin mutation {Method}, seat {SeatId}, caller {Identity}", method, seatId, name);
                            return notices.Cancel(seatId);
                        }
                        if (method == "seat.delete")
                        {
                            var target = seats.GetSeat(seatId) ?? throw new AdminFault("not_found", "Siège introuvable");
                            var purge = parameters.TryGetProperty("purgeAccount", out var purgeFlag) && purgeFlag.GetBoolean();
                            var result = await DeleteSeatAndAccountAsync(target, target.AccountName, sid, purge, ct);
                            logger.LogInformation("Kinseat admin mutation {Method}, seat {SeatId}, caller {Identity}", method, seatId, name);
                            return result;
                        }
                        var seat = seats.GetSeat(seatId) ?? throw new AdminFault("not_found", "Siège introuvable");
                        if (method is not "seat.preview.frame")
                            logger.LogInformation("Kinseat admin mutation {Method}, seat {SeatId}, caller {Identity}", method, seatId, name);
                        switch (method)
                        {
                            case "seat.graphics.set":
                                return await ApplyGraphicsAsync(seat, GraphicsSelection.Parse(parameters), ct);
                            case "seat.lightening.report": return lightening.Report(seatId, seat.AccountName, seat.Lightening);
                            case "seat.lightening.set":
                                seat.Lightening = new SeatLightening
                                {
                                    EmptyDesktop = parameters.GetProperty("emptyDesktop").GetBoolean(),
                                    DesktopBrowser = parameters.TryGetProperty("desktopBrowser", out var browser) && browser.GetBoolean(),
                                    BlockStartupApps = parameters.GetProperty("blockStartupApps").GetBoolean(),
                                    BlockVendorUtilities = parameters.GetProperty("blockVendorUtilities").GetBoolean(),
                                    CloseWindhawkTray = parameters.GetProperty("closeWindhawkTray").GetBoolean()
                                };
                                var saved = SeatPreset.ForAutoStart(seat);
                                saved.AutoStart = seat.AutoStart;
                                presets.Upsert(saved);
                                seats.TryApplyLightening(seat);
                                return seat.Lightening;
                            case "seat.clients.list": return seats.GetPairedClients(seatId);
                            case "seat.autostart.set":
                                SeatEndpoints.SetAutoStart(seat, parameters.GetProperty("enabled").GetBoolean(), presets);
                                return new { autoStart = seat.AutoStart };
                            case "seat.preset.set":
                            {
                                var quality = Enum.Parse<NvencQualityPreset>(parameters.GetProperty("preset").GetString()!);
                                try { await seats.SetNvencPresetAsync(seatId, quality, presets, ct); }
                                catch (InvalidOperationException ex) when (ex is not (SeatNotFoundException or ResourceConflictException))
                                { throw new AdminFault("unavailable", ex.Message); }
                                return new { preset = quality.ToString() };
                            }
                            case "seat.resolution.set":
                                try
                                {
                                    await seats.SetResolutionAsync(seatId, parameters.GetProperty("width").GetInt32(),
                                        parameters.GetProperty("height").GetInt32(), presets, ct);
                                }
                                catch (InvalidOperationException ex) when (ex is not (SeatNotFoundException or ResourceConflictException))
                                { throw new AdminFault("unavailable", ex.Message); }
                                return new { width = seat.Width, height = seat.Height, sessionId = seat.SessionId };
                            case "seat.repair":
                                try
                                {
                                    switch (parameters.GetProperty("target").GetString())
                                    {
                                        case "audio": await seats.ResetAudioAsync(seatId); break;
                                        case "display": await seats.ResetDisplayAsync(seatId, ct); break;
                                        default: await seats.ResetControllerAsync(seatId); break;
                                    }
                                }
                                catch (InvalidOperationException ex) when (ex is not (SeatNotFoundException or ResourceConflictException))
                                { throw new AdminFault("unavailable", ex.Message); }
                                return new { ok = true };
                            case "seat.clients.remove":
                                return seats.UnpairClient(seatId, parameters.GetProperty("name").GetString()!)
                                    ? new { ok = true }
                                    : throw new AdminFault("not_found", "Appareil introuvable");
                            case "seat.pair":
                                if (seat.Status is not (SeatStatus.Ready or SeatStatus.Streaming))
                                    throw new AdminFault("conflict", "Le siège n'est pas prêt");
                                try
                                {
                                    await seats.SubmitPairingPinAsync(seatId,
                                        parameters.GetProperty("pin").GetString()!,
                                        parameters.GetProperty("name").GetString()!, ct);
                                }
                                catch (InvalidOperationException ex) when (ex is not (SeatNotFoundException or ResourceConflictException))
                                {
                                    throw new AdminFault("unavailable", ex.Message);
                                }
                                return new { ok = true };
                            case "seat.notice.schedule":
                                return notices.Schedule(seat, parameters.GetProperty("minutes").GetInt32(),
                                    parameters.GetProperty("message").GetString()!);
                            case "seat.notice.cancel": return notices.Cancel(seatId);
                            case "seat.preview.start":
                                if (seat.Status is not (SeatStatus.Ready or SeatStatus.Streaming))
                                    throw new AdminFault("conflict", "Le siège n'est pas prêt");
                                return await previews.StartAsync(seatId, ct, name);
                            case "seat.preview.stop": return await previews.StopAsync(seatId, ct, name);
                            case "seat.preview.frame": return previews.Frame(seatId);
                            case "seat.action":
                                if (seat.Status is not (SeatStatus.Ready or SeatStatus.Streaming))
                                    throw new AdminFault("conflict", "Le siège n'est pas prêt");
                                switch (parameters.GetProperty("action").GetString())
                                {
                                    case "disconnect": await seats.StopApollo(seatId); break;
                                    case "restart_stream": await seats.RestartApolloAsync(seatId, ct); break;
                                    case "close": await seats.TeardownSeatAsync(seatId, ct); break;
                                }
                                return new { ok = true };
                            default: throw new AdminFault("unknown_method", "Méthode inconnue");
                        }
                    }, extraHandlers.SelectMany(h => h.Methods).ToHashSet(StringComparer.Ordinal)));
                var bytes = JsonSerializer.SerializeToUtf8Bytes(reply, Json);
                if (bytes.Length + 1 > MaxResponseBytes)
                    bytes = JsonSerializer.SerializeToUtf8Bytes(AdminDispatcher.Error(null, "response_too_large", "Réponse supérieure à 4 Mio"), Json);
                // Processing time (seat.create ~36 s) must not eat the time left to write the reply.
                timeout.CancelAfter(DefaultRequestTimeout);
                await pipe.WriteAsync(bytes, timeout.Token);
                await pipe.WriteAsync(new byte[] { (byte)'\n' }, timeout.Token);
                await pipe.FlushAsync(timeout.Token);
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested) { }
            catch (Exception ex)
            {
                logger.LogWarning(ex, "Kinseat admin pipe client failed");
            }
        }
    }

    private static async Task<byte[]?> ReadLineBoundedAsync(Stream stream, CancellationToken ct)
    {
        using var buffer = new MemoryStream();
        var one = new byte[1];
        while (buffer.Length < MaxLineBytes)
        {
            if (await stream.ReadAsync(one, ct) == 0) return null;
            if (one[0] == '\n') return buffer.ToArray();
            buffer.WriteByte(one[0]);
        }
        return null;
    }
}

/// <summary>Pure dispatcher: exercises the wire contract without opening a privileged pipe.</summary>
internal static class AdminDispatcher
{
    internal static async Task<object> DispatchAsync(byte[] line, Func<string, JsonElement, Task<object?>> execute,
        IReadOnlySet<string>? extraMethods = null)
    {
        ulong? requestId = null;
        try
        {
            using var doc = JsonDocument.Parse(line);
            var root = doc.RootElement;
            if (root.ValueKind != JsonValueKind.Object || !root.TryGetProperty("id", out var id)
                || !id.TryGetUInt64(out var parsedId)
                || !root.TryGetProperty("method", out var methodElement)
                || methodElement.ValueKind != JsonValueKind.String
                || !root.TryGetProperty("params", out var parameters)
                || parameters.ValueKind != JsonValueKind.Object
                || root.EnumerateObject().Any(p => p.Name is not ("id" or "method" or "params")))
                return Error(null, "invalid_request", "id, method et params requis");
            requestId = parsedId;
            var method = methodElement.GetString()!;
            var extra = extraMethods?.Contains(method) == true && method.StartsWith("library.", StringComparison.Ordinal);
            if (!extra && method is not ("system.health" or "seats.list" or "notices.list" or
                "seat.notice.schedule" or "seat.notice.cancel" or "seat.preview.start" or
                "seat.preview.stop" or "seat.preview.frame" or "seat.action" or "seat.pair" or
                "seat.clients.list" or "seat.clients.remove" or "settings.get" or "settings.set" or
                "accounts.list" or "account.create" or "account.delete" or "seat.create" or "seat.delete" or
                "diagnostics.export" or "diagnostics.getLevel" or "diagnostics.setLevel" or
                "host.refreshRate.get" or "host.refreshRate.set" or "seat.autostart.set" or
                "seat.preset.set" or "seat.resolution.set" or "seat.graphics.set" or "seat.repair" or
                "seat.lightening.set" or "seat.lightening.report" or
                "gpu.budget" or "gpu.budget.setReserve" or
                "app.authorization" or "app.authorize" or "app.revoke" or
                "joinRequests.list" or "joinRequest.accept" or "joinRequest.refuse" or
                "host.logonTasks.list" or "host.logonTasks.restrict" or "host.logonTasks.restore"))
                return Error(requestId, "unknown_method", "Méthode inconnue");
            if (extra)
            {
                // The handler owns its parameter schema; the pipe has already authorized the caller.
                var extensionResult = await execute(method, parameters);
                return extensionResult is null ? Error(requestId, "unavailable", "Résultat indisponible")
                    : new { id = requestId, result = extensionResult };
            }
            var allowed = method switch
            {
                "seat.notice.schedule" => new[] { "seatId", "minutes", "message" },
                "seat.action" => new[] { "seatId", "action" },
                "seat.pair" => new[] { "seatId", "pin", "name" },
                "seat.clients.remove" => new[] { "seatId", "name" },
                "seat.create" => new[] { "accountName" },
                "seat.delete" => new[] { "seatId" },
                "seat.graphics.set" => new[] { "seatId", "graphicsProfile" },
                "gpu.budget.setReserve" => new[] { "hostReserveMb" },
                "account.create" => new[] { "name" },
                "account.delete" => new[] { "accountName" },
                "settings.set" => new[] { "preview" },
                "diagnostics.setLevel" => new[] { "level" },
                "host.refreshRate.set" => new[] { "intervalMs" },
                "seat.autostart.set" => new[] { "seatId", "enabled" },
                "seat.preset.set" => new[] { "seatId", "preset" },
                "seat.lightening.set" => new[] { "seatId", "emptyDesktop", "blockStartupApps", "blockVendorUtilities", "closeWindhawkTray" },
                "seat.resolution.set" => new[] { "seatId", "width", "height" },
                "seat.repair" => new[] { "seatId", "target" },
                "joinRequest.accept" => new[] { "requestId", "seatId" },
                "joinRequest.refuse" => new[] { "requestId" },
                "host.logonTasks.restrict" or "host.logonTasks.restore" => new[] { "path", "name" },
                "system.health" or "seats.list" or "notices.list" or "settings.get" or
                    "gpu.budget" or
                    "accounts.list" or "diagnostics.export" or "diagnostics.getLevel" or
                    "host.refreshRate.get" or "app.authorization" or "app.authorize" or
                    "app.revoke" or "joinRequests.list" or "host.logonTasks.list" => Array.Empty<string>(),
                _ => new[] { "seatId" }
            };
            var permitted = method switch
            {
                "account.create" => new[] { "name", "password", "credential" },
                "seat.create" => new[] { "accountName", "displayName", "preset", "graphicsProfile", "vramBudgetMb", "width", "height", "fps", "nvencPreset" },
                "seat.delete" => new[] { "seatId", "purgeAccount" },
                "seat.lightening.set" => new[] { "seatId", "emptyDesktop", "desktopBrowser", "blockStartupApps", "blockVendorUtilities", "closeWindhawkTray" },
                "seat.graphics.set" => new[] { "seatId", "graphicsProfile", "vramBudgetMb", "width", "height", "fps", "nvencPreset" },
                "joinRequest.accept" => new[] { "requestId", "seatId", "graphicsProfile", "vramBudgetMb", "width", "height", "fps", "nvencPreset" },
                _ => allowed
            };
            if (parameters.EnumerateObject().Count() < allowed.Length ||
                parameters.EnumerateObject().Count() > permitted.Length ||
                parameters.EnumerateObject().Select(p => p.Name).Distinct().Count() != parameters.EnumerateObject().Count() ||
                parameters.EnumerateObject().Any(p => !permitted.Contains(p.Name)) ||
                allowed.Any(p => !parameters.TryGetProperty(p, out _)))
                return Error(requestId, "invalid_params", "Paramètres invalides");
            if ((method is "seat.create" or "seat.graphics.set" or "joinRequest.accept") &&
                (!GraphicsSelection.Valid(parameters, allowPartial: method != "seat.graphics.set",
                     allowNullBudget: method == "joinRequest.accept") ||
                 (method == "seat.create" && !parameters.TryGetProperty("graphicsProfile", out _) &&
                  (parameters.TryGetProperty("width", out _) || parameters.TryGetProperty("height", out _) ||
                   parameters.TryGetProperty("fps", out _) || parameters.TryGetProperty("nvencPreset", out _)))))
                return Error(requestId, "invalid_params", "Profil graphique invalide");
            if (method is "host.logonTasks.restrict" or "host.logonTasks.restore" &&
                (parameters.GetProperty("path").ValueKind != JsonValueKind.String ||
                 parameters.GetProperty("name").ValueKind != JsonValueKind.String ||
                 !LogonTaskManager.ValidPath(parameters.GetProperty("path").GetString()!,
                     parameters.GetProperty("name").GetString()!)))
                return Error(requestId, "invalid_params", "Chemin de tâche invalide");
            if (method == "gpu.budget.setReserve" &&
                !GraphicsSelection.IntRange(parameters.GetProperty("hostReserveMb"), 0, 262144))
                return Error(requestId, "invalid_params", "Réserve VRAM invalide");
            if (method == "seat.create" &&
                (parameters.GetProperty("accountName").ValueKind != JsonValueKind.String ||
                 !ApiInputValidation.IsValidAccountName(parameters.GetProperty("accountName").GetString()) ||
                 (parameters.TryGetProperty("displayName", out var displayName) &&
                  (displayName.ValueKind != JsonValueKind.String ||
                   displayName.GetString() is not { Length: >= 1 and <= 40 } label ||
                   string.IsNullOrWhiteSpace(label) || label.Any(char.IsControl))) ||
                 (parameters.TryGetProperty("preset", out var preset) &&
                  (preset.ValueKind != JsonValueKind.String ||
                   preset.GetString() is not ("Latency" or "Balanced" or "Quality")))))
                return Error(requestId, "invalid_params", "Place invalide");
            if (method == "account.delete" &&
                (parameters.GetProperty("accountName").ValueKind != JsonValueKind.String ||
                 !ApiInputValidation.IsValidAccountName(parameters.GetProperty("accountName").GetString())))
                return Error(requestId, "invalid_params", "Nom de compte invalide");
            if (method == "account.create" &&
                (parameters.GetProperty("name").ValueKind != JsonValueKind.String ||
                 !ApiInputValidation.IsValidAccountName(parameters.GetProperty("name").GetString()) ||
                 !ApiInputValidation.TryParseCredential(parameters, out _)))
                return Error(requestId, "invalid_params", "Compte invalide");
            if (method == "host.refreshRate.set" &&
                (parameters.GetProperty("intervalMs").ValueKind != JsonValueKind.Number ||
                 !parameters.GetProperty("intervalMs").TryGetInt32(out var interval) ||
                 !DwmFrameIntervalSetting.IsAllowedInterval(interval)))
                return Error(requestId, "invalid_params", "Fréquence non proposée");
            if (method == "seat.delete" && parameters.TryGetProperty("purgeAccount", out var purge) &&
                purge.ValueKind is not (JsonValueKind.True or JsonValueKind.False))
                return Error(requestId, "invalid_params", "purgeAccount doit être un booléen");
            if (method == "seat.autostart.set" &&
                parameters.GetProperty("enabled").ValueKind is not (JsonValueKind.True or JsonValueKind.False))
                return Error(requestId, "invalid_params", "Valeur invalide");
            if (method == "seat.lightening.set" &&
                (new[] { "emptyDesktop", "blockStartupApps", "blockVendorUtilities", "closeWindhawkTray" }
                    .Any(key => parameters.GetProperty(key).ValueKind is not (JsonValueKind.True or JsonValueKind.False)) ||
                 (parameters.TryGetProperty("desktopBrowser", out var browserFlag) &&
                  browserFlag.ValueKind is not (JsonValueKind.True or JsonValueKind.False))))
                return Error(requestId, "invalid_params", "Les réglages d'allègement doivent être des booléens");
            if (method == "seat.preset.set" &&
                (parameters.GetProperty("preset").ValueKind != JsonValueKind.String ||
                 parameters.GetProperty("preset").GetString() is not ("Latency" or "Balanced" or "Quality")))
                return Error(requestId, "invalid_params", "Qualité invalide");
            if (method == "seat.resolution.set" &&
                (parameters.GetProperty("width").ValueKind != JsonValueKind.Number ||
                 parameters.GetProperty("height").ValueKind != JsonValueKind.Number ||
                 !parameters.GetProperty("width").TryGetInt32(out var width) || width is < 640 or > 7680 ||
                 !parameters.GetProperty("height").TryGetInt32(out var height) || height is < 480 or > 4320))
                return Error(requestId, "invalid_params", "Résolution invalide");
            if (method == "seat.repair" &&
                (parameters.GetProperty("target").ValueKind != JsonValueKind.String ||
                 parameters.GetProperty("target").GetString() is not ("audio" or "display" or "controller")))
                return Error(requestId, "invalid_params", "Réparation inconnue");
            if (method == "diagnostics.setLevel" &&
                (parameters.GetProperty("level").ValueKind != JsonValueKind.String ||
                 parameters.GetProperty("level").GetString() is not ("Normal" or "Détaillé")))
                return Error(requestId, "invalid_params", "Niveau de journal invalide");
            if (method == "settings.set" &&
                (parameters.GetProperty("preview").ValueKind != JsonValueKind.Object ||
                 parameters.GetProperty("preview").EnumerateObject().Count() != 1 ||
                 !parameters.GetProperty("preview").TryGetProperty("showIndicator", out var indicator) ||
                 indicator.ValueKind is not (JsonValueKind.True or JsonValueKind.False)))
                return Error(requestId, "invalid_params", "Réglage d'aperçu invalide");
            if (method is "joinRequest.accept" or "joinRequest.refuse" &&
                (parameters.GetProperty("requestId").ValueKind != JsonValueKind.String ||
                 !Guid.TryParse(parameters.GetProperty("requestId").GetString(), out var joinId) || joinId == Guid.Empty))
                return Error(requestId, "invalid_params", "Identifiant de demande invalide");
            if (allowed.Contains("seatId") &&
                (parameters.GetProperty("seatId").ValueKind != JsonValueKind.String ||
                 !Guid.TryParse(parameters.GetProperty("seatId").GetString(), out var seatId) || seatId == Guid.Empty))
                return Error(requestId, "invalid_params", "Identifiant de siège invalide");
            if (method == "seat.notice.schedule" &&
                (!parameters.GetProperty("minutes").TryGetInt32(out var minutes) || minutes is < 1 or > 120 ||
                 parameters.GetProperty("message").ValueKind != JsonValueKind.String ||
                 parameters.GetProperty("message").GetString() is not { Length: >= 1 and <= 220 }))
                return Error(requestId, "invalid_params", "Durée ou message invalide");
            if (method == "seat.pair" &&
                (parameters.GetProperty("pin").ValueKind != JsonValueKind.String ||
                 parameters.GetProperty("name").ValueKind != JsonValueKind.String ||
                 !Streaming.ApolloPairingClient.Valid(parameters.GetProperty("pin").GetString(),
                     parameters.GetProperty("name").GetString())))
                return Error(requestId, "invalid_params", "PIN ou nom d'appareil invalide");
            if (method == "seat.clients.remove" &&
                (parameters.GetProperty("name").ValueKind != JsonValueKind.String ||
                 parameters.GetProperty("name").GetString() is not { Length: >= 1 and <= 40 } name ||
                 string.IsNullOrWhiteSpace(name) || name.Any(char.IsControl)))
                return Error(requestId, "invalid_params", "Nom d'appareil invalide");
            if (method == "seat.action" &&
                (parameters.GetProperty("action").ValueKind != JsonValueKind.String ||
                 parameters.GetProperty("action").GetString() is not ("disconnect" or "restart_stream" or "close")))
                return Error(requestId, "invalid_params", "Action inconnue");
            var result = await execute(method, parameters);
            return result is null ? Error(requestId, "unavailable", "Résultat indisponible") : new { id = requestId, result };
        }
        catch (JsonException) { return Error(requestId, "invalid_json", "JSON invalide"); }
        catch (PasswordPolicyException ex) { return Error(requestId, "password_policy", ex.Message); }
        catch (AdminFault ex) { return Error(requestId, ex.Code, ex.Message); }
        catch (SeatNotFoundException) { return Error(requestId, "not_found", "Siège introuvable"); }
        catch (ResourceConflictException) { return Error(requestId, "conflict", "Conflit de siège"); }
        catch (CapacityExhaustedException) { return Error(requestId, "conflict", "Limite de places atteinte"); }
        catch (ArgumentException) { return Error(requestId, "invalid_params", "Paramètres invalides"); }
        catch (InvalidOperationException) { return Error(requestId, "unavailable", "Opération indisponible : consultez le journal du service"); }
        catch (Exception) { return Error(requestId, "internal_error", "Erreur du service"); }
    }

    internal static object Error(ulong? id, string code, string message) =>
        new { id, error = new { code, message } };

    internal static object Dispatch(byte[] line, Func<string, object?> execute)
    {
        try
        {
            using var document = JsonDocument.Parse(line);
            var root = document.RootElement;
            if (root.ValueKind != JsonValueKind.Object
                || !root.TryGetProperty("id", out var idElement) || !idElement.TryGetUInt64(out var id)
                || !root.TryGetProperty("method", out var methodElement) || methodElement.ValueKind != JsonValueKind.String
                || !root.TryGetProperty("params", out var parameters) || parameters.ValueKind != JsonValueKind.Object
                || parameters.EnumerateObject().Any())
                return Error(null, "invalid_request", "id, method et params={} requis");
            var method = methodElement.GetString()!;
            if (method is not ("system.health" or "seats.list"))
                return Error(id, "unknown_method", "Méthode inconnue");
            var result = execute(method);
            return result is null ? Error(id, "unavailable", "Résultat indisponible") : new { id, result };
        }
        catch (JsonException) { return Error(null, "invalid_json", "JSON invalide"); }
        catch (Exception) { return Error(null, "internal_error", "Erreur du service"); }
    }
}

/// <summary>Turns a minimal-API result reused by the pipe into a value, or an admin error.</summary>
internal static class AdminResults
{
    internal static object? Unwrap(Microsoft.AspNetCore.Http.IResult result)
    {
        var status = (result as Microsoft.AspNetCore.Http.IStatusCodeHttpResult)?.StatusCode ?? 200;
        var value = (result as Microsoft.AspNetCore.Http.IValueHttpResult)?.Value;
        if (status < 400) return value;
        var message = "Opération refusée";
        if (value is not null)
        {
            var element = JsonSerializer.SerializeToElement(value);
            if (element.ValueKind == JsonValueKind.Object && element.TryGetProperty("error", out var e) &&
                e.ValueKind == JsonValueKind.String)
                message = e.GetString()!;
        }
        throw new AdminFault(status switch { 400 => "invalid_params", 404 => "not_found", 409 => "conflict", _ => "unavailable" }, message);
    }
}
