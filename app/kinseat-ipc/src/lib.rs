//! Protocole local d'administration Kinseat v1 (une ligne JSON UTF-8 par message).
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io;

pub const PROTOCOL_VERSION: u32 = 1;
pub const PIPE_NAME: &str = r"\\.\pipe\Kinseat.Admin.v1";
pub const MAX_LINE_BYTES: usize = 65_536;
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct Request<'a> {
    pub id: u64,
    pub method: &'a str,
    pub params: Value,
}

#[derive(Debug, Deserialize)]
pub struct Response {
    pub id: u64,
    pub result: Option<Value>,
    pub error: Option<ProtocolError>,
}

#[derive(Debug, Deserialize)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub active_seats: u32,
    pub max_seats: u32,
    pub health: HealthScore,
    pub windows_build: String,
    pub rdp_wrapper_active: bool,
    #[serde(default)]
    pub gpu: Option<Gpu>,
    #[serde(default)]
    pub cpu: Option<Cpu>,
    #[serde(default)]
    pub network: Option<Network>,
    #[serde(default)]
    pub top_processes: Vec<TopProcess>,
    #[serde(default)]
    pub disks: Vec<Disk>,
    #[serde(default)]
    pub system_memory_mb: u64,
    #[serde(default)]
    pub available_memory_mb: u64,
    #[serde(default)]
    pub timestamp: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gpu {
    pub name: String,
    pub utilization_percent: u32,
    pub vram_total_mb: u64,
    pub vram_used_mb: u64,
    pub encoder_utilization_percent: u32,
    pub active_encoder_sessions: u32,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cpu {
    pub name: String,
    pub utilization_percent: u32,
    pub logical_cores: u32,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Network {
    pub primary_adapter: String,
    pub bytes_received_per_sec: u64,
    pub bytes_sent_per_sec: u64,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopProcess {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f64,
    pub memory_mb: u64,
    pub network_connections: u32,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Disk {
    pub name: String,
    pub label: String,
    pub total_mb: u64,
    pub free_mb: u64,
    pub used_percent: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct HealthScore {
    pub status: String,
    pub score: u32,
    pub issues: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Seat {
    pub id: String,
    pub account_name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub status: String,
    pub session_id: i32,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub fps: u32,
    #[serde(default)]
    pub port_base: u32,
    #[serde(default, rename = "viGEmControllerIndex")]
    pub vi_gem_controller_index: Option<i32>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub ready_at: Option<String>,
    #[serde(default)]
    pub launch_app: Option<String>,
    #[serde(default)]
    pub auto_start: bool,
    /// Qualité d'encodage NVENC : `Latency`, `Balanced` ou `Quality`.
    #[serde(default)]
    pub nvenc_preset: Option<String>,
    #[serde(default)]
    pub provisioning_step: Option<String>,
    #[serde(default)]
    pub scale_factor: Option<u32>,
    #[serde(default)]
    pub effective_refresh_rate_warning: Option<String>,
    #[serde(default)]
    pub scale_note: Option<String>,
    #[serde(default)]
    pub graphics_profile: Option<String>,
    #[serde(default)]
    pub vram_budget_mb: u64,
    #[serde(default)]
    pub vram_used_mb: Option<u64>,
    #[serde(default)]
    pub vram_over_budget: bool,
    #[serde(default)]
    pub lightening: Lightening,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Lightening {
    pub empty_desktop: bool,
    pub block_startup_apps: bool,
    pub block_vendor_utilities: bool,
    pub close_windhawk_tray: bool,
}
impl Default for Lightening {
    fn default() -> Self {
        Self {
            empty_desktop: true,
            block_startup_apps: true,
            block_vendor_utilities: true,
            close_windhawk_tray: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuBudget {
    pub gpu_name: String,
    pub total_mb: u64,
    pub host_reserve_mb: u64,
    pub usable_mb: u64,
    pub allocated_mb: u64,
    pub free_mb: u64,
    pub seats: Vec<SeatBudget>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatBudget {
    pub seat_id: String,
    pub account_name: String,
    pub graphics_profile: String,
    pub vram_budget_mb: u64,
    pub vram_used_mb: Option<u64>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LighteningReport {
    pub settings: Lightening,
    pub stopped_processes: Vec<StoppedProcess>,
    pub blocked_startup_entries: Vec<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct StoppedProcess {
    pub name: String,
    pub category: String,
    pub at: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    pub games: Vec<GameEntry>,
    pub scanned_at: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameEntry {
    pub id: String,
    pub name: String,
    pub source: String,
    pub install_path: String,
    pub exe_path: Option<String>,
    pub launch_uri: Option<String>,
    pub size_bytes: Option<u64>,
    pub assigned_to: Vec<String>,
    #[serde(default)]
    pub access: std::collections::HashMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogonTask {
    pub path: String,
    pub name: String,
    pub execute: String,
    pub run_as: String,
    pub restricted: bool,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub exe_name: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteFailure {
    pub item: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteReport {
    pub ok: bool,
    pub removed: Vec<String>,
    pub failed: Vec<DeleteFailure>,
    pub account_kept: Option<String>,
}

/// Paramètres du profil ; les détails ne sont transmis que pour Custom.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphicsChoice {
    pub graphics_profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vram_budget_mb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nvenc_preset: Option<String>,
}
impl GraphicsChoice {
    pub fn preset(profile: &str) -> Self {
        Self {
            graphics_profile: profile.into(),
            vram_budget_mb: None,
            width: None,
            height: None,
            fps: None,
            nvenc_preset: None,
        }
    }
    pub fn budget_mb(&self) -> u64 {
        self.vram_budget_mb
            .unwrap_or(match self.graphics_profile.as_str() {
                "Economy" => 2048,
                "Quality" => 4096,
                _ => 3072,
            })
    }
}

pub fn graphics_request(id: u64, seat_id: &str, choice: &GraphicsChoice) -> Request<'static> {
    let mut params = serde_json::to_value(choice).expect("graphics parameters");
    params["seatId"] = serde_json::json!(seat_id);
    Request {
        id,
        method: "seat.graphics.set",
        params,
    }
}
pub fn seat_create_graphics_request(
    id: u64,
    account: &str,
    display: Option<&str>,
    choice: &GraphicsChoice,
) -> Request<'static> {
    let mut params = serde_json::to_value(choice).expect("graphics parameters");
    params["accountName"] = serde_json::json!(account);
    if let Some(display) = display {
        params["displayName"] = serde_json::json!(display);
    }
    Request {
        id,
        method: "seat.create",
        params,
    }
}
pub fn join_accept_request(
    id: u64,
    request_id: &str,
    seat_id: &str,
    choice: &GraphicsChoice,
) -> Request<'static> {
    let mut params = serde_json::to_value(choice).expect("graphics parameters");
    params["requestId"] = serde_json::json!(request_id);
    params["seatId"] = serde_json::json!(seat_id);
    Request {
        id,
        method: "joinRequest.accept",
        params,
    }
}
pub fn lightening_set_request(id: u64, seat_id: &str, settings: &Lightening) -> Request<'static> {
    let mut params = serde_json::to_value(settings).expect("lightening parameters");
    params["seatId"] = serde_json::json!(seat_id);
    Request {
        id,
        method: "seat.lightening.set",
        params,
    }
}
pub fn lightening_report_request(id: u64, seat_id: &str) -> Request<'static> {
    Request {
        id,
        method: "seat.lightening.report",
        params: serde_json::json!({"seatId":seat_id}),
    }
}
pub fn gpu_reserve_request(id: u64, host_reserve_mb: u64) -> Request<'static> {
    Request {
        id,
        method: "gpu.budget.setReserve",
        params: serde_json::json!({"hostReserveMb":host_reserve_mb}),
    }
}
pub fn library_request(id: u64, method: &'static str, params: Value) -> Request<'static> {
    Request { id, method, params }
}
pub fn library_add_request(id: u64, name: &str, exe_path: &str) -> Request<'static> {
    library_request(
        id,
        "library.add",
        serde_json::json!({"name":name,"exePath":exe_path}),
    )
}
pub fn library_remove_request(id: u64, game_id: &str) -> Request<'static> {
    library_request(id, "library.remove", serde_json::json!({"id":game_id}))
}
pub fn library_assign_request(
    id: u64,
    game_id: &str,
    account_name: &str,
    allowed: bool,
) -> Request<'static> {
    library_request(
        id,
        "library.assign",
        serde_json::json!({"gameId":game_id,"accountName":account_name,"allowed":allowed}),
    )
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub enum NoticePhase {
    Pending,
    Sending,
    Displayed,
    Closing,
    Closed,
    Failed,
    Cancelled,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub id: String,
    pub seat_id: String,
    pub account_name: String,
    pub message: String,
    pub minutes: u32,
    pub created_at_utc: String,
    pub deadline_utc: String,
    pub remaining_seconds: u64,
    pub phase: NoticePhase,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewFrame {
    pub active: bool,
    #[serde(default)]
    pub jpeg_base64: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub captured_at_utc: Option<String>,
    #[serde(default)]
    pub age_seconds: Option<f64>,
    pub live: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewSettings {
    pub preview: PreviewPreferences,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewPreferences {
    pub show_indicator: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticLevel {
    Normal,
    #[serde(rename = "Détaillé")]
    Detailed,
}

#[cfg(any(windows, test))]
#[derive(Debug, Deserialize)]
struct DiagnosticExport {
    path: String,
}

#[cfg(any(windows, test))]
#[derive(Debug, Deserialize)]
struct DiagnosticLevelReply {
    level: DiagnosticLevel,
}

#[cfg(windows)]
pub fn diagnostics_export() -> io::Result<String> {
    Ok(call::<DiagnosticExport>("diagnostics.export", 14)?.path)
}

#[cfg(windows)]
pub fn diagnostics_get_level() -> io::Result<DiagnosticLevel> {
    Ok(call::<DiagnosticLevelReply>("diagnostics.getLevel", 15)?.level)
}

#[cfg(windows)]
pub fn diagnostics_set_level(level: DiagnosticLevel) -> io::Result<DiagnosticLevel> {
    Ok(call_with::<DiagnosticLevelReply>(
        "diagnostics.setLevel",
        serde_json::json!({"level": level}),
        16,
    )?
    .level)
}

/// Ouverture de l'application sans élévation (autorisation donnée une fois par compte Windows).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct Authorization {
    pub authorized: bool,
    pub elevated: bool,
}

/// Le service a refusé l'appelant (compte non autorisé ou plus administrateur).
pub fn is_forbidden(error: &io::Error) -> bool {
    error.raw_os_error() == Some(5) || error.to_string().starts_with("forbidden:")
}

#[cfg(windows)]
pub fn app_authorization() -> io::Result<Authorization> {
    call("app.authorization", 40)
}

#[cfg(windows)]
pub fn app_authorize() -> io::Result<Authorization> {
    call("app.authorize", 41)
}

#[cfg(windows)]
pub fn app_revoke() -> io::Result<Authorization> {
    call("app.revoke", 42)
}

#[derive(Clone, Debug, Deserialize)]
pub struct ActionResult {
    pub ok: bool,
}

/// Fréquence de composition des places (intervalle DWM), commune à l'hôte.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshRate {
    pub interval_ms: u32,
    #[serde(default)]
    pub default_interval_ms: u32,
    #[serde(default)]
    pub allowed_intervals_ms: Vec<u32>,
}

/// Fréquence affichée pour un intervalle (même arrondi que le service : 16 ms → 62 Hz).
pub fn interval_hz(interval_ms: u32) -> u32 {
    1000 / interval_ms.max(1)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub username: String,
    pub is_managed: bool,
    #[serde(default)]
    pub has_seat: bool,
    #[serde(default)]
    pub managed: bool,
    #[serde(default)]
    pub deletable: bool,
    #[serde(default)]
    pub not_deletable_reason: Option<String>,
}

pub fn accounts_list_request(id: u64) -> Request<'static> {
    Request {
        id,
        method: "accounts.list",
        params: serde_json::json!({}),
    }
}
pub fn account_create_request<'a>(id: u64, name: &'a str, password: &'a str) -> Request<'a> {
    Request {
        id,
        method: "account.create",
        params: serde_json::json!({"name":name,"password":password}),
    }
}
pub fn seat_create_request<'a>(
    id: u64,
    account_name: &'a str,
    display_name: Option<&str>,
) -> Request<'a> {
    let mut params = serde_json::json!({"accountName":account_name});
    if let Some(display_name) = display_name {
        params["displayName"] = serde_json::json!(display_name);
    }
    Request {
        id,
        method: "seat.create",
        params,
    }
}
pub fn seat_delete_request(id: u64, seat_id: &str) -> Request<'_> {
    Request {
        id,
        method: "seat.delete",
        params: serde_json::json!({"seatId":seat_id}),
    }
}

#[cfg(windows)]
pub fn accounts_list() -> io::Result<Vec<Account>> {
    call("accounts.list", 20)
}
#[cfg(windows)]
pub fn account_create(name: &str, password: &str) -> io::Result<Account> {
    let r = account_create_request(21, name, password);
    call_with(r.method, r.params, r.id)
}
#[cfg(windows)]
pub fn seat_create(account_name: &str, display_name: Option<&str>) -> io::Result<Seat> {
    let r = seat_create_request(22, account_name, display_name);
    call_with(r.method, r.params, r.id)
}
#[cfg(windows)]
pub fn seat_delete(seat_id: &str) -> io::Result<ActionResult> {
    let r = seat_delete_request(23, seat_id);
    call_with(r.method, r.params, r.id)
}

pub fn seat_setting_request<'a>(
    id: u64,
    method: &'a str,
    seat_id: &str,
    mut params: serde_json::Value,
) -> Request<'a> {
    params["seatId"] = serde_json::json!(seat_id);
    Request { id, method, params }
}

pub fn pairing_request<'a>(id: u64, seat_id: &'a str, pin: &'a str, name: &'a str) -> Request<'a> {
    Request {
        id,
        method: "seat.pair",
        params: serde_json::json!({"seatId":seat_id,"pin":pin,"name":name}),
    }
}
pub fn clients_list_request(id: u64, seat_id: &str) -> Request<'_> {
    Request {
        id,
        method: "seat.clients.list",
        params: serde_json::json!({"seatId":seat_id}),
    }
}
pub fn clients_remove_request<'a>(id: u64, seat_id: &'a str, name: &'a str) -> Request<'a> {
    Request {
        id,
        method: "seat.clients.remove",
        params: serde_json::json!({"seatId":seat_id,"name":name}),
    }
}

#[cfg(windows)]
pub fn pair(seat_id: &str, pin: &str, name: &str) -> io::Result<ActionResult> {
    let request = pairing_request(17, seat_id, pin, name);
    call_with(request.method, request.params, request.id)
}
#[cfg(windows)]
pub fn clients_list(seat_id: &str) -> io::Result<Vec<String>> {
    let request = clients_list_request(18, seat_id);
    call_with(request.method, request.params, request.id)
}
#[cfg(windows)]
pub fn clients_remove(seat_id: &str, name: &str) -> io::Result<ActionResult> {
    let request = clients_remove_request(19, seat_id, name);
    call_with(request.method, request.params, request.id)
}

pub fn parse_response<T: serde::de::DeserializeOwned>(line: &[u8], id: u64) -> io::Result<T> {
    if line.len() > MAX_RESPONSE_BYTES || !line.ends_with(b"\n") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "trame trop longue ou incomplète",
        ));
    }
    let response: Response = serde_json::from_slice(line)?;
    if response.id != id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "identifiant de réponse incorrect",
        ));
    }
    match (response.result, response.error) {
        (Some(value), None) => serde_json::from_value(value).map_err(Into::into),
        (None, Some(error)) => Err(io::Error::other(format!(
            "{}: {}",
            error.code, error.message
        ))),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "réponse ambiguë",
        )),
    }
}

#[cfg(windows)]
pub fn call<T: serde::de::DeserializeOwned>(method: &str, id: u64) -> io::Result<T> {
    call_with(method, Value::Object(Default::default()), id)
}

#[cfg(windows)]
pub fn call_with<T: serde::de::DeserializeOwned>(
    method: &str,
    params: Value,
    id: u64,
) -> io::Result<T> {
    use std::io::{Read, Write};
    let mut pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(PIPE_NAME)?;
    let request = serde_json::to_vec(&Request { id, method, params })?;
    if request.len() + 1 > MAX_LINE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "requête trop longue",
        ));
    }
    pipe.write_all(&request)?;
    pipe.write_all(b"\n")?;
    let mut line = Vec::new();
    // Do not allocate unbounded data from an untrusted server.
    let mut reader = (&mut pipe).take((MAX_RESPONSE_BYTES + 1) as u64);
    loop {
        let mut byte = [0];
        if reader.read(&mut byte)? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "pipe fermé"));
        }
        line.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    parse_response(&line, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn join_accept_omits_null_and_sends_custom_fields() {
        let standard = join_accept_request(51, "req", "seat", &GraphicsChoice::preset("Quality"));
        assert_eq!(
            standard.params,
            serde_json::json!({"requestId":"req", "seatId":"seat", "graphicsProfile":"Quality"})
        );
        let custom = GraphicsChoice {
            graphics_profile: "Custom".into(),
            vram_budget_mb: Some(4096),
            width: Some(2560),
            height: Some(1440),
            fps: Some(60),
            nvenc_preset: Some("Quality".into()),
        };
        assert_eq!(
            join_accept_request(51, "req", "seat", &custom).params,
            serde_json::json!({"requestId":"req", "seatId":"seat", "graphicsProfile":"Custom", "vramBudgetMb":4096,
                "width":2560, "height":1440, "fps":60, "nvencPreset":"Quality"})
        );
    }
    #[test]
    fn authorization_reply_and_forbidden() {
        let reply: Authorization = parse_response(
            br#"{"id":41,"result":{"authorized":true,"elevated":true}}
"#,
            41,
        )
        .unwrap();
        assert_eq!(
            reply,
            Authorization {
                authorized: true,
                elevated: true
            }
        );
        let refused = parse_response::<Authorization>(
            br#"{"id":40,"error":{"code":"forbidden","message":"Acces"}}
"#,
            40,
        )
        .unwrap_err();
        assert!(is_forbidden(&refused));
        assert!(is_forbidden(&io::Error::from_raw_os_error(5)));
        assert!(!is_forbidden(&io::Error::from_raw_os_error(2)));
    }
    #[test]
    fn pairing_contract() {
        let seat = "00000000-0000-0000-0000-000000000001";
        assert_eq!(
            serde_json::to_value(pairing_request(17, seat, "0012", "Salon")).unwrap(),
            serde_json::json!({"id":17,"method":"seat.pair","params":{"seatId":seat,"pin":"0012","name":"Salon"}})
        );
        assert_eq!(
            serde_json::to_value(clients_list_request(18, seat)).unwrap(),
            serde_json::json!({"id":18,"method":"seat.clients.list","params":{"seatId":seat}})
        );
        assert_eq!(
            serde_json::to_value(clients_remove_request(19, seat, "Salon")).unwrap(),
            serde_json::json!({"id":19,"method":"seat.clients.remove","params":{"seatId":seat,"name":"Salon"}})
        );
        let list: Vec<String> =
            parse_response(b"{\"id\":18,\"result\":[\"Salon\"]}\n", 18).unwrap();
        assert_eq!(list, ["Salon"]);
        let removed: ActionResult =
            parse_response(b"{\"id\":19,\"result\":{\"ok\":true}}\n", 19).unwrap();
        assert!(removed.ok);
    }
    #[test]
    fn diagnostics_contract() {
        for (level, text) in [
            (DiagnosticLevel::Normal, "Normal"),
            (DiagnosticLevel::Detailed, "Détaillé"),
        ] {
            let request = serde_json::to_value(Request {
                id: 16,
                method: "diagnostics.setLevel",
                params: serde_json::json!({"level": level}),
            })
            .unwrap();
            assert_eq!(
                request,
                serde_json::json!({"id":16,"method":"diagnostics.setLevel","params":{"level":text}})
            );
            let reply: DiagnosticLevelReply = parse_response(
                format!("{{\"id\":16,\"result\":{{\"level\":\"{text}\"}}}}\n").as_bytes(),
                16,
            )
            .unwrap();
            assert_eq!(reply.level, level);
        }
        let export_request = serde_json::to_value(Request {
            id: 14,
            method: "diagnostics.export",
            params: serde_json::json!({}),
        })
        .unwrap();
        assert_eq!(export_request["params"], serde_json::json!({}));
        let export: DiagnosticExport = parse_response(
            b"{\"id\":14,\"result\":{\"path\":\"C:\\\\reports\\\\diagnostic.zip\"}}\n",
            14,
        )
        .unwrap();
        assert!(export.path.ends_with("diagnostic.zip"));
        let get: DiagnosticLevelReply =
            parse_response(b"{\"id\":15,\"result\":{\"level\":\"Normal\"}}\n", 15).unwrap();
        assert_eq!(get.level, DiagnosticLevel::Normal);
        assert!(parse_response::<DiagnosticLevelReply>(
            b"{\"id\":15,\"result\":{\"level\":\"invalid\"}}\n",
            15
        )
        .is_err());
    }
    #[test]
    fn preview_setting_contract() {
        let settings: PreviewSettings = parse_response(
            b"{\"id\":12,\"result\":{\"preview\":{\"showIndicator\":false}}}\n",
            12,
        )
        .unwrap();
        assert!(!settings.preview.show_indicator);
        let payload = serde_json::to_value(Request {
            id: 13,
            method: "settings.set",
            params: serde_json::json!({"preview":{"showIndicator":false}}),
        })
        .unwrap();
        assert_eq!(payload["params"]["preview"]["showIndicator"], false);
    }
    #[test]
    fn request_serialization() {
        let json = serde_json::to_value(Request {
            id: 3,
            method: "seats.list",
            params: serde_json::json!({}),
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"id":3,"method":"seats.list","params":{}})
        );
    }
    #[test]
    fn seat_fixture_from_http_shape() {
        let seats: Vec<Seat> = parse_response(b"{\"id\":2,\"result\":[{\"id\":\"3f2e\",\"accountName\":\"player\",\"status\":\"Streaming\",\"sessionId\":4,\"width\":1920,\"height\":1080,\"errorMessage\":null}]}\n", 2).unwrap();
        assert_eq!(seats[0].status, "Streaming");
    }
    #[test]
    fn full_pipe_fixtures_and_optional_fields() {
        let seats: Vec<Seat> = parse_response(concat!(r#"{"id":3,"result":[{"id":"d9c7","accountName":"Camille","sessionId":3,"status":"Streaming","width":1920,"height":1080,"fps":60,"portBase":48100,"viGEmControllerIndex":-1,"createdAt":"2026-11-13T18:00:00Z","readyAt":null,"errorMessage":null,"launchApp":"Rocket League","autoStart":true,"provisioningStep":null,"scaleFactor":125,"effectiveRefreshRateWarning":null,"scaleNote":null},{"id":"e9","accountName":"Léa","sessionId":-1,"status":"Recovering","width":1280,"height":720,"errorMessage":null}]}"#, "\n").as_bytes(), 3).unwrap();
        assert_eq!(seats.len(), 2);
        assert_eq!(seats[0].vi_gem_controller_index, Some(-1));
        assert_eq!(seats[0].launch_app.as_deref(), Some("Rocket League"));
        assert_eq!(seats[1].fps, 0);
        assert_eq!(seats[1].status, "Recovering");
        let health: Health = parse_response(concat!(r#"{"id":4,"result":{"activeSeats":2,"maxSeats":4,"gpu":{"name":"RTX 3080","utilizationPercent":65,"vramTotalMb":10240,"vramUsedMb":6500,"encoderUtilizationPercent":32,"activeEncoderSessions":2},"cpu":{"name":"Ryzen","utilizationPercent":74,"logicalCores":16},"network":{"primaryAdapter":"Ethernet","bytesReceivedPerSec":1200000,"bytesSentPerSec":3000000},"topProcesses":[{"pid":123,"name":"Jeu","cpuPercent":13.5,"memoryMb":800,"networkConnections":2}],"disks":[{"name":"C:","label":"Système","totalMb":1024000,"freeMb":200000,"usedPercent":80}],"health":{"status":"Warning","score":85,"issues":["CPU high: 78%"]},"systemMemoryMb":32768,"availableMemoryMb":12000,"windowsBuild":"Windows 11","rdpWrapperActive":true,"timestamp":"2026-11-13T18:30:00Z"}}"#, "\n").as_bytes(), 4).unwrap();
        assert_eq!(health.gpu.as_ref().unwrap().active_encoder_sessions, 2);
        assert_eq!(
            health.network.as_ref().unwrap().bytes_sent_per_sec,
            3_000_000
        );
        assert_eq!(health.top_processes[0].cpu_percent, 13.5);
        assert_eq!(health.disks[0].used_percent, 80);
    }
    #[test]
    fn health_fixture_and_errors() {
        let h: Health = parse_response(b"{\"id\":1,\"result\":{\"activeSeats\":1,\"maxSeats\":4,\"health\":{\"status\":\"Healthy\",\"score\":100,\"issues\":[]},\"windowsBuild\":\"Windows\",\"rdpWrapperActive\":true}}\n", 1).unwrap();
        assert_eq!(h.health.score, 100);
        assert!(parse_response::<Health>(b"{\"id\":1,\"error\":{\"code\":\"unknown_method\",\"message\":\"Non pris en charge\"}}\n", 1).is_err());
        assert!(parse_response::<Health>(b"{\"id\":2,\"result\":{}}\n", 1).is_err());
        assert!(parse_response::<Health>(b"{}", 1).is_err());
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn seat_setting_adds_seat_id() {
        let r = seat_setting_request(
            5,
            "seat.preset.set",
            "abc",
            serde_json::json!({"preset": "Quality"}),
        );
        assert_eq!(r.method, "seat.preset.set");
        assert_eq!(
            r.params,
            serde_json::json!({"preset": "Quality", "seatId": "abc"})
        );
    }

    #[test]
    fn refresh_rate_parses_service_status() {
        let r: RefreshRate = serde_json::from_str(
            r#"{"intervalMs":8,"refreshRateHz":125,"registryIntervalMs":8,"defaultIntervalMs":8,"allowedIntervalsMs":[33,16,8,6],"appliesTo":"x"}"#,
        )
        .unwrap();
        assert_eq!(r.allowed_intervals_ms, vec![33, 16, 8, 6]);
        assert_eq!(interval_hz(r.interval_ms), 125);
        assert_eq!(interval_hz(16), 62);
    }

    #[test]
    fn seat_reads_preset() {
        let seat: Seat = serde_json::from_str(
            r#"{"id":"a","accountName":"lea","status":"Ready","sessionId":3,"width":1920,"height":1080,"nvencPreset":"Latency"}"#,
        )
        .unwrap();
        assert_eq!(seat.nvenc_preset.as_deref(), Some("Latency"));
    }
    #[test]
    fn reads_gpu_budget_reply() {
        let budget: GpuBudget = parse_response(&[br#"{"id":5,"result":{"gpuName":"RTX","totalMb":10240,"hostReserveMb":2048,"usableMb":8192,"allocatedMb":3072,"freeMb":5120,"seats":[{"seatId":"s1","accountName":"Lea","graphicsProfile":"Balanced","vramBudgetMb":3072,"vramUsedMb":null}]}}"#.as_slice(), b"\n"].concat(), 5).unwrap();
        assert_eq!(budget.host_reserve_mb, 2048);
        assert_eq!(budget.seats[0].vram_used_mb, None);
        assert_eq!(budget.seats[0].seat_id, "s1");
    }

    #[test]
    fn reads_library_list_reply() {
        let library: Library = parse_response(&[br#"{"id":6,"result":{"games":[{"id":"g1","name":"Test","source":"Steam","installPath":"C:\\Games","exePath":null,"launchUri":"steam://run/1","sizeBytes":123,"assignedTo":["lea"]}],"scannedAt":null}}"#.as_slice(), b"\n"].concat(), 6).unwrap();
        assert_eq!(library.games[0].assigned_to, ["lea"]);
        assert_eq!(library.games[0].source, "Steam");
        assert!(library.scanned_at.is_none());
    }

    #[test]
    fn reads_lightening_report_reply() {
        let report: LighteningReport = parse_response(&[br#"{"id":7,"result":{"settings":{"emptyDesktop":true,"blockStartupApps":false,"blockVendorUtilities":true,"closeWindhawkTray":false},"stoppedProcesses":[{"name":"tray","category":"vendor","at":"2026-11-18T12:00:00Z"}],"blockedStartupEntries":["updater"]}}"#.as_slice(), b"\n"].concat(), 7).unwrap();
        assert!(!report.settings.block_startup_apps);
        assert_eq!(report.stopped_processes[0].category, "vendor");
        assert_eq!(report.blocked_startup_entries, ["updater"]);
    }
}

#[cfg(test)]
mod preview_frame_tests {
    use super::PreviewFrame;

    #[test]
    fn accepts_fractional_age_seconds_from_service() {
        let json = r#"{"active":true,"jpegBase64":"AA==","width":1280,"height":720,
            "capturedAtUtc":"2026-11-18T20:44:00Z","ageSeconds":0.2166275,"live":true}"#;
        let frame: PreviewFrame = serde_json::from_str(json).expect("fractional age must parse");
        assert!(frame.live);
        assert!((frame.age_seconds.unwrap() - 0.2166275).abs() < 1e-9);
    }
}
