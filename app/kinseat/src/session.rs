use crate::logging;
use kinseat_ipc::{DiagnosticLevel, Notice, PreviewFrame};
use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

pub const DEFAULT_MESSAGE: &str = "La session va se terminer. Pense à sauvegarder ta partie.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Credential {
    Pin(String),
    None,
    Password(String),
}

impl Credential {
    fn ipc_value(&self) -> serde_json::Value {
        match self {
            Self::Pin(value) => serde_json::json!({"kind":"pin","value":value}),
            Self::None => serde_json::json!({"kind":"none"}),
            Self::Password(value) => serde_json::json!({"kind":"password","value":value}),
        }
    }
}

fn account_create_params(name: String, credential: Credential) -> serde_json::Value {
    serde_json::json!({"name":name,"credential":credential.ipc_value()})
}

pub enum Command {
    Refresh,
    AccountsList,
    AccountCreate {
        name: String,
        credential: Credential,
    },
    SeatCreate {
        account: String,
        display_name: String,
        graphics: kinseat_ipc::GraphicsChoice,
    },
    GpuBudget,
    GpuReserve(u64),
    LighteningReport(String),
    LibraryList,
    LibraryScan,
    LibraryAdd {
        name: String,
        exe_path: String,
    },
    LibraryRemove(String),
    LibraryAssign {
        game_id: String,
        account: String,
        allowed: bool,
    },
    #[allow(dead_code)] // Câblé avec le dialogue de bibliothèque dans l'étape suivante.
    LibraryGrant {
        game_id: String,
        account: String,
    },
    LibrarySetOptions {
        game_id: String,
        start_from_host_save: bool,
    },
    AccountDelete(String),
    SeatDelete {
        seat: String,
        purge_account: bool,
    },
    LogonTasksList,
    LogonTaskChange {
        path: String,
        name: String,
        restore: bool,
    },
    RefreshRateGet,
    RefreshRateSet(u32),
    /// Réglage ou réparation d'une place : méthode du canal et paramètres (sans `seatId`).
    SeatSetting {
        seat: String,
        method: &'static str,
        params: serde_json::Value,
    },
    ClientsList(String),
    JoinAccept {
        request_id: String,
        seat_id: String,
        graphics: kinseat_ipc::GraphicsChoice,
    },
    JoinRefuse(String),
    Pair {
        seat: String,
        pin: String,
        name: String,
    },
    ClientsRemove {
        seat: String,
        name: String,
    },
    SettingsGet,
    SettingsSet(bool),
    DiagnosticsGetLevel,
    DiagnosticsSetLevel(DiagnosticLevel),
    AuthorizationGet,
    AuthorizationRevoke,
    DiagnosticsExport,
    Schedule {
        seat: String,
        minutes: u32,
        message: String,
    },
    Cancel(String),
    PreviewStart(String),
    PreviewStop(String),
    Action {
        seat: String,
        action: &'static str,
    },
}

pub enum Event {
    Snapshot {
        seats: io::Result<(
            Vec<kinseat_ipc::Seat>,
            std::collections::HashMap<String, bool>,
        )>,
        health: io::Result<kinseat_ipc::Health>,
        notices: io::Result<Vec<Notice>>,
    },
    Accounts(io::Result<Vec<kinseat_ipc::Account>>),
    AccountCreated(io::Result<kinseat_ipc::Account>),
    AccountDeleted(io::Result<kinseat_ipc::DeleteReport>),
    SeatCreated(io::Result<kinseat_ipc::Seat>),
    GpuBudget(io::Result<kinseat_ipc::GpuBudget>),
    LighteningReport(String, io::Result<kinseat_ipc::LighteningReport>),
    Library(
        io::Result<(
            kinseat_ipc::Library,
            std::collections::HashMap<String, crate::library::SaveOptions>,
        )>,
    ),
    LibraryChanged(String, io::Result<serde_json::Value>),
    SeatDeleted(io::Result<kinseat_ipc::DeleteReport>),
    LogonTasks(io::Result<Vec<kinseat_ipc::LogonTask>>),
    LogonTaskChanged(String, bool, io::Result<serde_json::Value>),
    RefreshRate(io::Result<kinseat_ipc::RefreshRate>),
    SeatSetting(&'static str, io::Result<serde_json::Value>),
    Notice(io::Result<Notice>),
    PreviewStarted(String, io::Result<()>),
    PreviewFrame(String, io::Result<PreviewFrame>),
    Action(String, String, io::Result<()>),
    Clients(String, io::Result<Vec<String>>),
    JoinRequests(io::Result<Vec<crate::join::JoinRequest>>),
    /// Réponse à une demande : (identifiant, acceptée ?, résultat).
    JoinAnswered(String, bool, io::Result<()>),
    Pair(io::Result<()>),
    ClientsRemoved(io::Result<()>),
    Settings(io::Result<kinseat_ipc::PreviewSettings>),
    DiagnosticsLevel(io::Result<DiagnosticLevel>),
    Authorization(io::Result<kinseat_ipc::Authorization>),
    DiagnosticsExport(io::Result<String>),
}

pub fn start(ctx: egui::Context) -> (Sender<Command>, Receiver<Event>) {
    let (commands, rx) = mpsc::channel();
    let (tx, events) = mpsc::channel();
    std::thread::spawn(move || worker(rx, tx, ctx));
    (commands, events)
}

fn worker(rx: Receiver<Command>, tx: Sender<Event>, ctx: egui::Context) {
    let mut preview: Option<String> = None;
    let mut refresh_at = Instant::now();
    let mut frame_at = Instant::now();
    let mut pending = None;
    let mut join_supported = true;
    send(
        &tx,
        &ctx,
        Event::LogonTasks(call("host.logonTasks.list", 70)),
    );
    loop {
        let command = pending.take().or_else(|| rx.try_recv().ok());
        if let Some(command) = command {
            match command {
                Command::Refresh => refresh_at = Instant::now(),
                Command::AccountsList => {
                    send(&tx, &ctx, Event::Accounts(call("accounts.list", 20)))
                }
                Command::AccountCreate { name, credential } => {
                    send(
                        &tx,
                        &ctx,
                        Event::AccountCreated(call_with(
                            "account.create",
                            account_create_params(name, credential),
                            21,
                        )),
                    );
                }
                Command::SeatCreate {
                    account,
                    display_name,
                    graphics,
                } => {
                    let r = kinseat_ipc::seat_create_graphics_request(
                        22,
                        &account,
                        (!display_name.is_empty()).then_some(display_name.as_str()),
                        &graphics,
                    );
                    send(
                        &tx,
                        &ctx,
                        Event::SeatCreated(call_with(r.method, r.params, r.id)),
                    );
                    refresh_at = Instant::now();
                }
                Command::GpuBudget => send(&tx, &ctx, Event::GpuBudget(call("gpu.budget", 60))),
                Command::GpuReserve(value) => {
                    let r = kinseat_ipc::gpu_reserve_request(61, value);
                    send(
                        &tx,
                        &ctx,
                        Event::GpuBudget(call_with(r.method, r.params, r.id)),
                    );
                }
                Command::LighteningReport(seat) => {
                    let r = kinseat_ipc::lightening_report_request(62, &seat);
                    send(
                        &tx,
                        &ctx,
                        Event::LighteningReport(seat, call_with(r.method, r.params, r.id)),
                    );
                }
                Command::LibraryList | Command::LibraryScan => {
                    let method = if matches!(command, Command::LibraryScan) {
                        "library.scan"
                    } else {
                        "library.list"
                    };
                    let result: io::Result<serde_json::Value> = call(method, 63);
                    send(
                        &tx,
                        &ctx,
                        Event::Library(result.and_then(|value| {
                            crate::library::parse_library(value).map_err(io::Error::other)
                        })),
                    );
                }
                Command::LibraryAdd { name, exe_path } => {
                    let r = kinseat_ipc::library_add_request(64, &name, &exe_path);
                    send(
                        &tx,
                        &ctx,
                        Event::LibraryChanged(
                            crate::library::ADD_KEY.into(),
                            call_with(r.method, r.params, r.id),
                        ),
                    );
                }
                Command::LibraryRemove(game_id) => {
                    let r = kinseat_ipc::library_remove_request(65, &game_id);
                    send(
                        &tx,
                        &ctx,
                        Event::LibraryChanged(game_id, call_with(r.method, r.params, r.id)),
                    );
                }
                Command::LibraryAssign {
                    game_id,
                    account,
                    allowed,
                } => {
                    let r = kinseat_ipc::library_assign_request(66, &game_id, &account, allowed);
                    send(
                        &tx,
                        &ctx,
                        Event::LibraryChanged(game_id, call_with(r.method, r.params, r.id)),
                    );
                }
                Command::LibraryGrant { game_id, account } => {
                    let result = call_with(
                        "library.grantAccess",
                        serde_json::json!({"gameId":game_id,"accountName":account}),
                        67,
                    );
                    send(&tx, &ctx, Event::LibraryChanged(game_id, result));
                }
                Command::LibrarySetOptions {
                    game_id,
                    start_from_host_save,
                } => {
                    let result = call_with(
                        "library.setOptions",
                        serde_json::json!({"gameId":game_id,"startFromHostSave":start_from_host_save}),
                        68,
                    );
                    send(&tx, &ctx, Event::LibraryChanged(game_id, result));
                }
                Command::AccountDelete(account) => {
                    send(
                        &tx,
                        &ctx,
                        Event::AccountDeleted(call_with(
                            "account.delete",
                            serde_json::json!({"accountName":account}),
                            24,
                        )),
                    );
                    refresh_at = Instant::now();
                }
                Command::SeatDelete {
                    seat,
                    purge_account,
                } => {
                    send(
                        &tx,
                        &ctx,
                        Event::SeatDeleted(call_with(
                            "seat.delete",
                            serde_json::json!({"seatId":seat,"purgeAccount":purge_account}),
                            23,
                        )),
                    );
                    refresh_at = Instant::now();
                }
                Command::LogonTasksList => send(
                    &tx,
                    &ctx,
                    Event::LogonTasks(call("host.logonTasks.list", 70)),
                ),
                Command::LogonTaskChange {
                    path,
                    name,
                    restore,
                } => {
                    let method = if restore {
                        "host.logonTasks.restore"
                    } else {
                        "host.logonTasks.restrict"
                    };
                    let result =
                        call_with(method, serde_json::json!({"path":path,"name":name}), 71);
                    send(&tx, &ctx, Event::LogonTaskChanged(name, restore, result));
                }
                Command::ClientsList(seat) => send(
                    &tx,
                    &ctx,
                    Event::Clients(
                        seat.clone(),
                        call_with("seat.clients.list", serde_json::json!({"seatId":seat}), 18),
                    ),
                ),
                Command::JoinAccept {
                    request_id,
                    seat_id,
                    graphics,
                } => {
                    let r = kinseat_ipc::join_accept_request(51, &request_id, &seat_id, &graphics);
                    let result: io::Result<serde_json::Value> = call_with(r.method, r.params, r.id);
                    send(
                        &tx,
                        &ctx,
                        Event::JoinAnswered(request_id, true, result.map(|_| ())),
                    );
                    refresh_at = Instant::now();
                }
                Command::JoinRefuse(request_id) => {
                    let result: io::Result<serde_json::Value> = call_with(
                        "joinRequest.refuse",
                        serde_json::json!({"requestId":request_id}),
                        52,
                    );
                    send(
                        &tx,
                        &ctx,
                        Event::JoinAnswered(request_id, false, result.map(|_| ())),
                    );
                    refresh_at = Instant::now();
                }
                Command::Pair { seat, pin, name } => {
                    let result: io::Result<kinseat_ipc::ActionResult> = call_with(
                        "seat.pair",
                        serde_json::json!({"seatId":seat,"pin":pin,"name":name}),
                        17,
                    );
                    send(
                        &tx,
                        &ctx,
                        Event::Pair(result.and_then(|r| {
                            if r.ok {
                                Ok(())
                            } else {
                                Err(io::Error::other("Appairage refusé"))
                            }
                        })),
                    );
                    send(
                        &tx,
                        &ctx,
                        Event::Clients(
                            seat.clone(),
                            call_with("seat.clients.list", serde_json::json!({"seatId":seat}), 18),
                        ),
                    );
                }
                Command::ClientsRemove { seat, name } => {
                    let result: io::Result<kinseat_ipc::ActionResult> = call_with(
                        "seat.clients.remove",
                        serde_json::json!({"seatId":seat,"name":name}),
                        19,
                    );
                    send(
                        &tx,
                        &ctx,
                        Event::ClientsRemoved(result.and_then(|r| {
                            if r.ok {
                                Ok(())
                            } else {
                                Err(io::Error::other("Retrait refusé"))
                            }
                        })),
                    );
                    send(
                        &tx,
                        &ctx,
                        Event::Clients(
                            seat.clone(),
                            call_with("seat.clients.list", serde_json::json!({"seatId":seat}), 18),
                        ),
                    );
                }
                Command::DiagnosticsGetLevel => {
                    send(&tx, &ctx, Event::DiagnosticsLevel(diagnostics_get_level()))
                }
                Command::AuthorizationGet => send(
                    &tx,
                    &ctx,
                    Event::Authorization(call("app.authorization", 40)),
                ),
                Command::AuthorizationRevoke => {
                    send(&tx, &ctx, Event::Authorization(call("app.revoke", 42)))
                }
                Command::DiagnosticsSetLevel(level) => send(
                    &tx,
                    &ctx,
                    Event::DiagnosticsLevel(diagnostics_set_level(level)),
                ),
                Command::DiagnosticsExport => {
                    send(&tx, &ctx, Event::DiagnosticsExport(diagnostics_export()))
                }
                Command::SettingsGet => send(&tx, &ctx, Event::Settings(call("settings.get", 12))),
                Command::RefreshRateGet => send(
                    &tx,
                    &ctx,
                    Event::RefreshRate(call("host.refreshRate.get", 30)),
                ),
                Command::RefreshRateSet(interval) => {
                    let result = call_with::<serde_json::Value>(
                        "host.refreshRate.set",
                        serde_json::json!({"intervalMs": interval}),
                        31,
                    )
                    .and_then(|_| call("host.refreshRate.get", 30));
                    send(&tx, &ctx, Event::RefreshRate(result));
                }
                Command::SeatSetting {
                    seat,
                    method,
                    params,
                } => {
                    let r = kinseat_ipc::seat_setting_request(32, method, &seat, params);
                    send(
                        &tx,
                        &ctx,
                        Event::SeatSetting(method, call_with(r.method, r.params, r.id)),
                    );
                    refresh_at = Instant::now();
                }
                Command::SettingsSet(enabled) => send(
                    &tx,
                    &ctx,
                    Event::Settings(call_with(
                        "settings.set",
                        serde_json::json!({"preview":{"showIndicator":enabled}}),
                        13,
                    )),
                ),
                Command::Schedule {
                    seat,
                    minutes,
                    message,
                } => {
                    send(
                        &tx,
                        &ctx,
                        Event::Notice(call_with(
                            "seat.notice.schedule",
                            serde_json::json!({"seatId":seat,"minutes":minutes,"message":message}),
                            4,
                        )),
                    );
                    refresh_at = Instant::now();
                }
                Command::Cancel(seat) => {
                    send(
                        &tx,
                        &ctx,
                        Event::Notice(call_with(
                            "seat.notice.cancel",
                            serde_json::json!({"seatId":seat}),
                            5,
                        )),
                    );
                    refresh_at = Instant::now();
                }
                Command::PreviewStart(seat) => {
                    if let Some(old) = preview.take() {
                        let _: io::Result<serde_json::Value> =
                            call_with("seat.preview.stop", serde_json::json!({"seatId":old}), 6);
                    }
                    let result: io::Result<serde_json::Value> =
                        call_with("seat.preview.start", serde_json::json!({"seatId":seat}), 7);
                    let result = result.and_then(|v| {
                        if v.get("active").and_then(|a| a.as_bool()) == Some(true) {
                            Ok(())
                        } else {
                            Err(io::Error::other("capture non activée"))
                        }
                    });
                    if result.is_ok() {
                        preview = Some(seat.clone());
                        frame_at = Instant::now();
                    }
                    send(&tx, &ctx, Event::PreviewStarted(seat, result));
                }
                Command::PreviewStop(seat) => {
                    if preview.as_deref() == Some(&seat) {
                        preview = None;
                    }
                    let _: io::Result<serde_json::Value> =
                        call_with("seat.preview.stop", serde_json::json!({"seatId":seat}), 8);
                }
                Command::Action { seat, action } => {
                    let result: io::Result<kinseat_ipc::ActionResult> = call_with(
                        "seat.action",
                        serde_json::json!({"seatId":seat,"action":action}),
                        9,
                    );
                    send(
                        &tx,
                        &ctx,
                        Event::Action(
                            seat,
                            action.into(),
                            result.and_then(|r| {
                                if r.ok {
                                    Ok(())
                                } else {
                                    Err(io::Error::other("action refusée"))
                                }
                            }),
                        ),
                    );
                    refresh_at = Instant::now();
                }
            }
            continue;
        }
        if Instant::now() >= refresh_at {
            send(
                &tx,
                &ctx,
                Event::Snapshot {
                    seats: call::<serde_json::Value>("seats.list", 1).and_then(parse_seats),
                    health: call("system.health", 2),
                    notices: call("notices.list", 3),
                },
            );
            if join_supported {
                let list: io::Result<serde_json::Value> = call("joinRequests.list", 50);
                match list {
                    // Service plus ancien que l'application : on n'interroge plus, sans bruit.
                    Err(e) if crate::join::is_unknown_method(&e.to_string()) => {
                        join_supported = false
                    }
                    other => send(
                        &tx,
                        &ctx,
                        Event::JoinRequests(other.map(crate::join::parse_list)),
                    ),
                }
            }
            refresh_at = Instant::now() + Duration::from_secs(2);
        }
        if let Some(seat) = &preview {
            if Instant::now() >= frame_at {
                send(
                    &tx,
                    &ctx,
                    Event::PreviewFrame(
                        seat.clone(),
                        call_with("seat.preview.frame", serde_json::json!({"seatId":seat}), 11),
                    ),
                );
                frame_at = Instant::now() + Duration::from_millis(500);
            }
        }
        let next = if preview.is_some() {
            refresh_at.min(frame_at)
        } else {
            refresh_at
        };
        match rx.recv_timeout(next.saturating_duration_since(Instant::now())) {
            Ok(cmd) => pending = Some(cmd),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(seat) = preview {
                    let _: io::Result<serde_json::Value> =
                        call_with("seat.preview.stop", serde_json::json!({"seatId":seat}), 10);
                }
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn parse_seats(
    value: serde_json::Value,
) -> io::Result<(
    Vec<kinseat_ipc::Seat>,
    std::collections::HashMap<String, bool>,
)> {
    let browser = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|seat| {
            Some((
                seat.get("id")?.as_str()?.to_owned(),
                seat.pointer("/lightening/desktopBrowser")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            ))
        })
        .collect();
    let seats = serde_json::from_value(value).map_err(io::Error::other)?;
    Ok((seats, browser))
}

fn send(tx: &Sender<Event>, ctx: &egui::Context, event: Event) {
    if matches!(
        &event,
        Event::Snapshot { health: Err(_), .. }
            | Event::DiagnosticsLevel(Err(_))
            | Event::DiagnosticsExport(Err(_))
            | Event::Settings(Err(_))
            | Event::Action(_, _, Err(_))
            | Event::Notice(Err(_))
    ) {
        logging::record("ERROR", "Erreur IPC (détails non consignés)");
    }
    let _ = tx.send(event);
    ctx.request_repaint();
}

#[cfg(windows)]
fn diagnostics_export() -> io::Result<String> {
    kinseat_ipc::diagnostics_export()
}
#[cfg(not(windows))]
fn diagnostics_export() -> io::Result<String> {
    Err(io::Error::other("service Windows indisponible"))
}
#[cfg(windows)]
fn diagnostics_get_level() -> io::Result<DiagnosticLevel> {
    kinseat_ipc::diagnostics_get_level()
}
#[cfg(not(windows))]
fn diagnostics_get_level() -> io::Result<DiagnosticLevel> {
    Err(io::Error::other("service Windows indisponible"))
}
#[cfg(windows)]
fn diagnostics_set_level(level: DiagnosticLevel) -> io::Result<DiagnosticLevel> {
    kinseat_ipc::diagnostics_set_level(level)
}
#[cfg(not(windows))]
fn diagnostics_set_level(_: DiagnosticLevel) -> io::Result<DiagnosticLevel> {
    Err(io::Error::other("service Windows indisponible"))
}

#[cfg(windows)]
fn call<T: serde::de::DeserializeOwned>(method: &str, id: u64) -> io::Result<T> {
    kinseat_ipc::call(method, id)
}
#[cfg(not(windows))]
fn call<T: serde::de::DeserializeOwned>(_: &str, _: u64) -> io::Result<T> {
    Err(io::Error::other("service Windows indisponible"))
}
#[cfg(windows)]
fn call_with<T: serde::de::DeserializeOwned>(
    method: &str,
    params: serde_json::Value,
    id: u64,
) -> io::Result<T> {
    kinseat_ipc::call_with(method, params, id)
}
#[cfg(not(windows))]
fn call_with<T: serde::de::DeserializeOwned>(
    _: &str,
    _: serde_json::Value,
    _: u64,
) -> io::Result<T> {
    Err(io::Error::other("service Windows indisponible"))
}

#[cfg(test)]
mod credential_tests {
    use super::{account_create_params, Credential};
    use serde_json::json;

    #[test]
    fn account_create_credentials_have_exact_wire_shape() {
        assert_eq!(
            account_create_params("lea".into(), Credential::Pin("0123".into())),
            json!({"name":"lea","credential":{"kind":"pin","value":"0123"}})
        );
        assert_eq!(
            account_create_params("lea".into(), Credential::None),
            json!({"name":"lea","credential":{"kind":"none"}})
        );
        assert_eq!(
            account_create_params("lea".into(), Credential::Password("passphrase".into())),
            json!({"name":"lea","credential":{"kind":"password","value":"passphrase"}})
        );
    }
}
