#![cfg_attr(windows, windows_subsystem = "windows")]
mod demo;
mod errors;
mod fonts;
mod graphics;
mod join;
mod library;
mod logging;
mod logon_tasks;
mod multisession;
mod notice;
mod overview;
mod preferences;
mod session;
mod theme;
#[allow(dead_code)] // Ancien rendu conservé pour les autres helpers de vues, non appelé.
mod views;

use kinseat_ipc::{DiagnosticLevel, Health, NoticePhase, PreviewFrame, Seat};
use session::{Command, Event};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CredentialMode {
    #[default]
    Pin,
    None,
    Password,
}

impl CredentialMode {
    fn credential(self, value: String) -> session::Credential {
        match self {
            Self::Pin => session::Credential::Pin(value),
            Self::None => session::Credential::None,
            Self::Password => session::Credential::Password(value),
        }
    }

    fn validate(self, value: &str, confirmation: &str) -> Result<(), &'static str> {
        match self {
            Self::Pin if value.len() != 4 || !value.bytes().all(|c| c.is_ascii_digit()) => {
                Err("Le code doit contenir exactement 4 chiffres.")
            }
            Self::Password if value.chars().count() < 8 => {
                Err("Mot de passe trop court : 8 caractères minimum.")
            }
            Self::None => Ok(()),
            Self::Pin if value != confirmation => Err("Les deux codes ne correspondent pas."),
            Self::Password if value != confirmation => {
                Err("Les deux mots de passe ne correspondent pas.")
            }
            _ => Ok(()),
        }
    }
}

/// Nom de compte proposé depuis le pseudo du Client, sans modifier les comptes existants.
fn derive_member_name(pseudo: &str, existing: &[String]) -> String {
    let mut base = String::new();
    for c in pseudo.chars() {
        let replacement = match c {
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => "a",
            'À' | 'Á' | 'Â' | 'Ä' | 'Ã' | 'Å' => "A",
            'é' | 'è' | 'ê' | 'ë' => "e",
            'É' | 'È' | 'Ê' | 'Ë' => "E",
            'î' | 'ï' | 'ì' | 'í' => "i",
            'Î' | 'Ï' | 'Ì' | 'Í' => "I",
            'ô' | 'ö' | 'ò' | 'ó' | 'õ' => "o",
            'Ô' | 'Ö' | 'Ò' | 'Ó' | 'Õ' => "O",
            'ù' | 'ú' | 'û' | 'ü' => "u",
            'Ù' | 'Ú' | 'Û' | 'Ü' => "U",
            'ÿ' | 'ý' => "y",
            'Ÿ' | 'Ý' => "Y",
            'ç' => "c",
            'Ç' => "C",
            'ñ' => "n",
            'Ñ' => "N",
            'œ' => "oe",
            'Œ' => "OE",
            'æ' => "ae",
            'Æ' => "AE",
            _ if c.is_ascii_alphanumeric() || c == '-' || c == '_' => {
                base.push(c);
                continue;
            }
            _ => continue,
        };
        base.push_str(replacement);
        if base.len() >= 20 {
            break;
        }
    }
    base.truncate(20);
    if base.is_empty() {
        base = "joueur".into();
    }
    let mut candidate = base.clone();
    let mut suffix = 2;
    while existing
        .iter()
        .any(|name| name.eq_ignore_ascii_case(&candidate))
    {
        let tail = suffix.to_string();
        candidate = format!("{}{}", &base[..base.len().min(20 - tail.len())], tail);
        suffix += 1;
    }
    candidate
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JoinCreateStage {
    Member,
    Seat,
    Accept,
    RetrySeat,
}

fn member_credentials(
    ui: &mut egui::Ui,
    mode: &mut CredentialMode,
    pw: &mut String,
    pw2: &mut String,
) {
    ui.label("Identifiant du joueur");
    let previous_mode = *mode;
    egui::ComboBox::from_id_salt("member-credential-mode")
        .selected_text(match mode {
            CredentialMode::Pin => "Code à 4 chiffres",
            CredentialMode::None => "Sans mot de passe",
            CredentialMode::Password => "Mot de passe",
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(mode, CredentialMode::Pin, "Code à 4 chiffres");
            ui.selectable_value(mode, CredentialMode::None, "Sans mot de passe");
            ui.selectable_value(mode, CredentialMode::Password, "Mot de passe");
        });
    if *mode != previous_mode {
        pw.clear();
        pw2.clear();
    }
    match mode {
        CredentialMode::Pin => {
            ui.label("Ce code est le mot de passe Windows du compte : le joueur peut ouvrir sa session manuellement sur ce PC.");
            ui.label("Code à 4 chiffres");
            ui.add(
                egui::TextEdit::singleline(pw)
                    .password(true)
                    .desired_width(330.0)
                    .char_limit(4),
            );
            ui.label("Confirmer le code");
            ui.add(
                egui::TextEdit::singleline(pw2)
                    .password(true)
                    .desired_width(330.0)
                    .char_limit(4),
            );
        }
        CredentialMode::Password => {
            ui.label("Mot de passe");
            ui.add(
                egui::TextEdit::singleline(pw)
                    .password(true)
                    .desired_width(330.0)
                    .char_limit(127),
            );
            ui.label("Confirmer le mot de passe");
            ui.add(
                egui::TextEdit::singleline(pw2)
                    .password(true)
                    .desired_width(330.0)
                    .char_limit(127),
            );
        }
        CredentialMode::None => {
            ui.label("Le joueur n'aura aucun mot de passe à retenir : Kinseat ouvre sa session. Il ne pourra pas ouvrir ce compte directement sur l'écran de connexion de Windows.");
        }
    }
}

fn valid_member_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 20
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn account_create_error(error: &str) -> String {
    if error.split(':').next() == Some("password_policy") {
        "La stratégie de mot de passe de ce PC exige un mot de passe plus long. Choisissez « Mot de passe » ou « Sans mot de passe ».".into()
    } else {
        notice::seat_error(error)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Overview,
    Seats,
    Preview,
    Notice,
    Members,
    Devices,
    Library,
    Settings,
}
impl Page {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "overview" => Some(Self::Overview),
            "seats" => Some(Self::Seats),
            "preview" => Some(Self::Preview),
            "notice" => Some(Self::Notice),
            "members" => Some(Self::Members),
            "devices" => Some(Self::Devices),
            "library" => Some(Self::Library),
            "settings" => Some(Self::Settings),
            _ => None,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Overview => "Vue d’ensemble",
            Page::Seats | Page::Preview | Page::Notice => "Places",
            Self::Members => "Membres",
            Self::Devices => "Appareils",
            Self::Library => "Bibliothèque",
            Self::Settings => "Paramètres",
        }
    }
}

struct Fault {
    kind: errors::Kind,
    detail: String,
}
struct Kinseat {
    page: Page,
    seats: Vec<Seat>,
    health: Option<Health>,
    notices: Vec<notice::TimedNotice>,
    fault: Option<Fault>,
    pending: bool,
    tx: Option<Sender<Command>>,
    rx: Option<Receiver<Event>>,
    demo: bool,
    demo_ipc_delay: Duration,
    demo_commands: Vec<(Instant, Command)>,
    preview: Option<String>,
    preview_frame: Option<PreviewFrame>,
    preview_texture: Option<egui::TextureHandle>,
    preview_error: Option<String>,
    preview_started: Instant,
    /// Recette : ouvrir l'aperçu de la première place active puis capturer une image réelle.
    capture_preview: bool,
    schedule: Option<String>,
    minutes: String,
    message: String,
    modal_error: Option<String>,
    confirm_close: Option<String>,
    clients: HashMap<String, Vec<String>>,
    pairing: bool,
    pair_seat: String,
    pair_name: String,
    pair_pin: String,
    pair_pending: bool,
    remove_client: Option<(String, String)>,
    accounts: Option<Vec<kinseat_ipc::Account>>,
    members_error: Option<String>,
    /// Dialogue « Ajouter une place » : (compte choisi, nom affiché).
    add_seat: Option<(String, String)>,
    delete_seat: Option<String>,
    purge_account: bool,
    delete_report: Option<kinseat_ipc::DeleteReport>,
    delete_retry_id: Option<String>,
    created_highlight: Option<(String, Instant)>,
    join_after_create: Option<(String, kinseat_ipc::GraphicsChoice)>,
    join_new_member: bool,
    join_create_stage: Option<JoinCreateStage>,
    logon_tasks: Option<Vec<kinseat_ipc::LogonTask>>,
    logon_confirm: Option<Vec<(String, String, bool)>>,
    logon_selected: Vec<String>,
    logon_details: bool,
    logon_report: Vec<String>,
    dismissed_logon_tasks: Vec<String>,
    logon_error: Option<String>,
    delete_member: Option<String>,
    delete_member_checked: bool,
    member_delete_report: Option<kinseat_ipc::DeleteReport>,
    member_delete_retry: Option<String>,
    /// Dialogue « Ajouter un membre » : (nom, mode justificatif, valeur, confirmation).
    new_member: Option<(String, CredentialMode, String, String)>,
    seat_pending: bool,
    seat_actions_pending: HashMap<String, &'static str>,
    accounts_requested: bool,
    refresh_rate: Option<kinseat_ipc::RefreshRate>,
    refresh_rate_error: Option<String>,
    refresh_rate_pending: bool,
    /// Fenêtre « Réglages de la place » : identifiant de la place.
    seat_settings: Option<String>,
    seat_settings_status: Option<(bool, String)>,
    /// Résolution saisie : (largeur, hauteur).
    seat_resolution: (u32, u32),
    devices_error: Option<String>,
    screenshot: Option<PathBuf>,
    screenshot_requested: bool,
    screenshot_frames: u8,
    screenshot_error: Option<String>,
    show_indicator: Option<bool>,
    settings_pending: bool,
    settings_error: Option<String>,
    diagnostic_level: Option<DiagnosticLevel>,
    diagnostic_pending: bool,
    authorization: Option<kinseat_ipc::Authorization>,
    authorization_pending: bool,
    authorization_message: Option<(String, bool)>,
    diagnostic_error: Option<String>,
    export_pending: bool,
    export_path: Option<String>,
    export_error: Option<String>,
    multisession: multisession::Panel,
    join: join::Panel,
    library: library::Panel,
    gpu_budget: Option<kinseat_ipc::GpuBudget>,
    /// Join request for which the VRAM budget was last fetched (the dialog shows the gauge).
    budget_for_join: Option<String>,
    gpu_error: Option<String>,
    graphics_choice: kinseat_ipc::GraphicsChoice,
    default_profile: String,
    reduced_motion: bool,
    reserve_mb: u64,
    lightening_report: Option<kinseat_ipc::LighteningReport>,
    lightening_draft: kinseat_ipc::Lightening,
    desktop_browser: HashMap<String, bool>,
    desktop_browser_draft: bool,
    page_transition: Instant,
}
impl Kinseat {
    fn new(
        cc: &eframe::CreationContext<'_>,
        page: Page,
        demo: bool,
        screenshot: Option<PathBuf>,
        open_logon_tasks: bool,
        search: String,
    ) -> Self {
        theme::install(&cc.egui_ctx);
        fonts::install(&cc.egui_ctx);
        let prefs = preferences::load();
        let (tx, rx) = if demo {
            (None, None)
        } else {
            let (tx, rx) = session::start(cc.egui_ctx.clone());
            (Some(tx), Some(rx))
        };
        let (health, seats, notices) = if demo {
            let (health, mut seats) = demo::data();
            // Variante de capture : KINSEAT_DEMO_SEATS=0..8 (données fictives).
            if let Some(n) = std::env::var("KINSEAT_DEMO_SEATS")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
            {
                let base = seats.clone();
                seats = (0..n)
                    .filter_map(|i| {
                        let mut seat = base.get(i % base.len().max(1))?.clone();
                        if i >= base.len() {
                            seat.id = format!("{}-{i}", seat.id);
                            seat.account_name =
                                ["Noé", "Inès", "Sami", "Jade", "Malo"][i % 5].into();
                        }
                        Some(seat)
                    })
                    .collect();
            }
            (Some(health), seats, demo::notices())
        } else {
            (None, Vec::new(), Vec::new())
        };
        let preview = if page == Page::Preview && demo {
            seats.first().map(|s| s.id.clone())
        } else {
            None
        };
        let clients = if demo {
            seats
                .iter()
                .map(|seat| {
                    (
                        seat.id.clone(),
                        vec![format!("Appareil fictif · {}", seat.account_name)],
                    )
                })
                .collect()
        } else {
            HashMap::new()
        };
        Self {
            page: if matches!(page, Page::Preview | Page::Notice) {
                Page::Seats
            } else {
                page
            },
            seats,
            health,
            notices: notices.into_iter().map(notice::TimedNotice::new).collect(),
            fault: None,
            multisession: multisession::Panel::new(demo),
            join: join::Panel::default(),
            library: library::Panel {
                data: demo.then(demo::library),
                search,
                ..Default::default()
            },
            gpu_budget: demo.then(demo::budget),
            budget_for_join: None,
            gpu_error: None,
            graphics_choice: kinseat_ipc::GraphicsChoice::preset(&prefs.default_profile),
            default_profile: prefs.default_profile,
            reduced_motion: prefs.reduced_motion,
            reserve_mb: prefs.reserve_mb,
            lightening_report: None,
            lightening_draft: kinseat_ipc::Lightening::default(),
            desktop_browser: HashMap::new(),
            desktop_browser_draft: false,
            page_transition: Instant::now(),
            pending: !demo,
            tx,
            rx,
            demo,
            demo_ipc_delay: if demo {
                Duration::from_millis(
                    std::env::var("KINSEAT_DEMO_IPC_DELAY_MS")
                        .ok()
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or(0),
                )
            } else {
                Duration::ZERO
            },
            demo_commands: Vec::new(),
            preview,
            preview_frame: None,
            preview_texture: None,
            preview_error: None,
            preview_started: Instant::now(),
            capture_preview: false,
            schedule: if page == Page::Notice && demo {
                Some("1".into())
            } else {
                None
            },
            minutes: "15".into(),
            message: session::DEFAULT_MESSAGE.into(),
            modal_error: None,
            confirm_close: None,
            clients,
            pairing: false,
            pair_seat: String::new(),
            accounts: None,
            members_error: None,
            add_seat: None,
            delete_seat: None,
            purge_account: false,
            delete_report: None,
            delete_retry_id: None,
            created_highlight: None,
            join_after_create: None,
            join_new_member: false,
            join_create_stage: None,
            logon_tasks: None,
            logon_confirm: None,
            logon_selected: Vec::new(),
            logon_details: open_logon_tasks,
            logon_report: Vec::new(),
            dismissed_logon_tasks: prefs.dismissed_logon_tasks,
            logon_error: None,
            delete_member: None,
            delete_member_checked: false,
            member_delete_report: None,
            member_delete_retry: None,
            new_member: None,
            seat_pending: false,
            seat_actions_pending: HashMap::new(),
            accounts_requested: false,
            refresh_rate: None,
            refresh_rate_error: None,
            refresh_rate_pending: false,
            seat_settings: None,
            seat_settings_status: None,
            seat_resolution: (1920, 1080),
            pair_name: String::new(),
            pair_pin: String::new(),
            pair_pending: false,
            remove_client: None,
            devices_error: None,
            screenshot,
            screenshot_requested: false,
            screenshot_frames: 0,
            screenshot_error: None,
            show_indicator: if demo { Some(true) } else { None },
            settings_pending: false,
            settings_error: None,
            diagnostic_level: if demo {
                Some(DiagnosticLevel::Normal)
            } else {
                None
            },
            diagnostic_pending: false,
            authorization: if demo {
                Some(kinseat_ipc::Authorization {
                    authorized: true,
                    elevated: false,
                })
            } else {
                None
            },
            authorization_pending: false,
            authorization_message: None,
            diagnostic_error: None,
            export_pending: false,
            export_path: None,
            export_error: None,
        }
    }
    fn dispatch(&mut self, command: Command) {
        if let Command::Action { seat, action } = &command {
            if self.seat_actions_pending.contains_key(seat) {
                return;
            }
            self.seat_actions_pending.insert(seat.clone(), action);
        }
        if self.demo {
            if !self.demo_ipc_delay.is_zero()
                && matches!(
                    command,
                    Command::LibraryAdd { .. }
                        | Command::LibraryRemove(_)
                        | Command::LibraryAssign { .. }
                        | Command::LibraryGrant { .. }
                        | Command::LibrarySetOptions { .. }
                        | Command::AccountDelete(_)
                        | Command::SeatDelete { .. }
                        | Command::Action { .. }
                )
            {
                self.demo_commands
                    .push((Instant::now() + self.demo_ipc_delay, command));
            } else {
                self.demo_command(command);
            }
        } else if let Some(tx) = &self.tx {
            if tx.send(command).is_err() {
                self.modal_error = Some("Connexion au service interrompue".into());
            }
        }
    }
    fn demo_command(&mut self, command: Command) {
        match command {
            Command::ClientsList(_) => {}
            Command::JoinAccept { request_id, .. } => {
                self.join.demo_set_status(&request_id, "paired")
            }
            Command::JoinRefuse(request_id) => self.join.demo_set_status(&request_id, "refused"),
            Command::AccountsList => {
                let taken: Vec<String> =
                    self.seats.iter().map(|s| s.account_name.clone()).collect();
                let mut names: Vec<String> = taken.clone();
                names.push("Invité".into());
                self.accounts = Some(
                    names
                        .into_iter()
                        .filter_map(|n| {
                            serde_json::from_value(serde_json::json!({"username": n, "isManaged": true, "managed": true, "deletable": true, "hasSeat": taken.contains(&n)})).ok()
                        })
                        .collect(),
                );
            }
            Command::AccountCreate { name, .. } => {
                if self.join_create_stage == Some(JoinCreateStage::Member) {
                    self.join_create_stage = None;
                    self.modal_error =
                        Some("Démonstration : aucun membre ni place n'est réellement créé.".into());
                    self.add_seat = None;
                    self.join_after_create = None;
                    self.new_member = None;
                    return;
                }
                if let Some(list) = self.accounts.as_mut() {
                    if let Ok(a) = serde_json::from_value(
                        serde_json::json!({"username": name, "isManaged": true, "managed": true, "deletable": true, "hasSeat": false}),
                    ) {
                        list.push(a);
                    }
                }
                self.new_member = None;
            }
            Command::GpuBudget => {}
            Command::GpuReserve(value) => {
                self.reserve_mb = value;
                self.gpu_budget = Some(demo::budget_with_reserve(value));
            }
            Command::LighteningReport(_) => {
                self.lightening_report = Some(demo::lightening_report());
            }
            Command::LibraryList | Command::LibraryScan => {
                self.library.data = Some(demo::library());
                self.library.loading = false;
            }
            Command::LibraryAdd { name, exe_path } => {
                if let Some(data) = &mut self.library.data {
                    data.games.push(serde_json::from_value(serde_json::json!({"id":format!("manual:demo-{}", data.games.len()),"name":name,"source":"manual","installPath":exe_path,"exePath":exe_path,"assignedTo":[]})).expect("demo game"));
                }
                self.library.loading = false;
                self.library.pending.finish(library::ADD_KEY);
            }
            Command::LibraryRemove(id) => {
                if let Some(data) = &mut self.library.data {
                    data.games.retain(|g| g.id != id);
                }
                self.library.pending.finish(&id);
            }
            Command::LibraryAssign {
                game_id,
                account,
                allowed,
            } => {
                if let Some(game) = self
                    .library
                    .data
                    .as_mut()
                    .and_then(|d| d.games.iter_mut().find(|g| g.id == game_id))
                {
                    game.assigned_to.retain(|a| a != &account);
                    if allowed {
                        game.assigned_to.push(account);
                    }
                }
                self.library.pending.finish(&game_id);
            }
            Command::LibrarySetOptions {
                game_id,
                start_from_host_save,
            } => {
                self.library
                    .options
                    .entry(game_id.clone())
                    .or_insert_with(|| library::SaveOptions {
                        save_isolation: "auto".into(),
                        ..Default::default()
                    })
                    .start_from_host_save = start_from_host_save;
                self.library.pending.finish(&game_id);
            }
            Command::SeatCreate { .. } => {
                self.add_seat = None;
                self.modal_error =
                    Some("Démonstration : aucune place n’est réellement créée.".into());
            }
            Command::RefreshRateGet | Command::RefreshRateSet(_) => {
                let current = match command {
                    Command::RefreshRateSet(ms) => ms,
                    _ => self.refresh_rate.as_ref().map_or(16, |r| r.interval_ms),
                };
                self.refresh_rate = serde_json::from_value(serde_json::json!({
                    "intervalMs": current, "defaultIntervalMs": 16, "allowedIntervalsMs": [33, 16, 8, 6]
                }))
                .ok();
            }
            Command::SeatSetting {
                seat,
                method,
                params,
            } => {
                if let Some(s) = self.seats.iter_mut().find(|s| s.id == seat) {
                    match method {
                        "seat.autostart.set" => {
                            s.auto_start = params["enabled"].as_bool().unwrap_or(false)
                        }
                        "seat.preset.set" => {
                            s.nvenc_preset = params["preset"].as_str().map(str::to_owned)
                        }
                        "seat.graphics.set" => {
                            s.graphics_profile =
                                params["graphicsProfile"].as_str().map(str::to_owned);
                            s.vram_budget_mb =
                                params["vramBudgetMb"].as_u64().unwrap_or_else(|| {
                                    kinseat_ipc::GraphicsChoice::preset(
                                        s.graphics_profile.as_deref().unwrap_or("Balanced"),
                                    )
                                    .budget_mb()
                                });
                        }
                        "seat.lightening.set" => {
                            if let Ok(value) = serde_json::from_value(params.clone()) {
                                s.lightening = value;
                            }
                        }
                        "seat.resolution.set" => {
                            s.width = params["width"].as_u64().unwrap_or(1920) as u32;
                            s.height = params["height"].as_u64().unwrap_or(1080) as u32;
                        }
                        _ => {}
                    }
                }
                self.seat_settings_status = Some((true, "Démonstration : réglage simulé.".into()));
            }
            Command::SeatDelete { seat: id, .. } => {
                self.seats.retain(|s| s.id != id);
                self.delete_seat = None;
                self.delete_report = serde_json::from_value(serde_json::json!({"ok":true,"removed":["preset","account"],"failed":[],"accountKept":null})).ok();
                self.seat_pending = false;
            }
            Command::LogonTasksList => {}
            Command::LogonTaskChange {
                path,
                name,
                restore,
            } => {
                if let Some(task) = self
                    .logon_tasks
                    .as_mut()
                    .and_then(|tasks| tasks.iter_mut().find(|t| t.path == path && t.name == name))
                {
                    task.restricted = !restore;
                }
            }
            Command::Pair { seat, pin: _, name } => {
                self.clients.entry(seat).or_default().push(name);
                self.pairing = false;
                self.pair_pending = false;
                self.pair_pin.clear();
            }
            Command::ClientsRemove { seat, name } => {
                if let Some(list) = self.clients.get_mut(&seat) {
                    list.retain(|n| n != &name);
                }
            }
            Command::Schedule {
                seat,
                minutes,
                message,
            } => {
                if self
                    .notices
                    .iter()
                    .any(|n| n.active() && n.notice.seat_id == seat)
                {
                    self.modal_error = Some(notice::service_error("conflict"));
                    return;
                }
                let account = self
                    .seats
                    .iter()
                    .find(|s| s.id == seat)
                    .map(|s| s.account_name.clone())
                    .unwrap_or_default();
                let value = serde_json::json!({"id":"demo-new","seatId":seat,"accountName":account,"message":message,"minutes":minutes,"createdAtUtc":"2026-11-13T18:00:00Z","deadlineUtc":"2026-11-13T18:15:00Z","remainingSeconds":minutes * 60,"phase":"Displayed"});
                if let Ok(n) = serde_json::from_value(value) {
                    self.notices.insert(0, notice::TimedNotice::new(n));
                    self.schedule = None;
                }
            }
            Command::Cancel(seat) => {
                if let Some(n) = self
                    .notices
                    .iter_mut()
                    .find(|n| n.active() && n.notice.seat_id == seat)
                {
                    if n.notice.phase != NoticePhase::Closing {
                        n.notice.phase = NoticePhase::Cancelled;
                    }
                }
            }
            Command::Action { seat, action } => {
                self.seat_actions_pending.remove(&seat);
                if let Some(s) = self.seats.iter_mut().find(|s| s.id == seat) {
                    if action == "close" {
                        s.status = "Idle".into();
                    } else if action == "disconnect" {
                        s.status = "Ready".into();
                    } else {
                        s.status = "Streaming".into();
                    }
                }
            }
            Command::SettingsGet => {}
            Command::SettingsSet(value) => self.show_indicator = Some(value),
            Command::DiagnosticsGetLevel => {}
            Command::DiagnosticsSetLevel(level) => self.diagnostic_level = Some(level),
            Command::AuthorizationGet => {}
            Command::AuthorizationRevoke => {
                self.authorization = Some(kinseat_ipc::Authorization {
                    authorized: false,
                    elevated: false,
                });
                self.authorization_pending = false;
            }
            Command::DiagnosticsExport => {
                self.export_path = Some("C:\\Kinseat\\diagnostic-demo.zip".into());
                self.export_pending = false;
            }
            Command::AccountDelete(account) => {
                self.seats.retain(|seat| seat.account_name != account);
                if let Some(accounts) = &mut self.accounts {
                    accounts.retain(|a| a.username != account);
                }
                self.delete_member = None;
                self.member_delete_retry = None;
                self.member_delete_report = serde_json::from_value(serde_json::json!({"ok":true,"removed":["seat","shortcuts","account"],"failed":[],"accountKept":null})).ok();
                self.seat_pending = false;
            }
            Command::LibraryGrant { game_id, .. } => self.library.pending.finish(&game_id),
            Command::PreviewStart(_) | Command::PreviewStop(_) | Command::Refresh => {}
        }
    }
    fn refresh(&mut self, _ctx: &egui::Context) {
        self.dispatch(Command::Refresh);
    }
    fn read_events(&mut self, ctx: &egui::Context) {
        while let Some(event) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            match event {
                Event::Snapshot {
                    seats,
                    health,
                    notices,
                } => {
                    logging::record("DEBUG", "Actualisation de l'état du service");
                    self.pending = false;
                    match health {
                        Ok(h) => {
                            self.health = Some(h);
                            self.fault = None;
                        }
                        Err(e) => {
                            self.health = None;
                            self.fault = Some(Fault {
                                kind: errors::classify(&e),
                                detail: e.to_string(),
                            });
                        }
                    }
                    match seats {
                        Ok((s, browser)) => {
                            self.desktop_browser = browser;
                            if self.page == Page::Devices && !self.demo {
                                for seat in &s {
                                    self.dispatch(Command::ClientsList(seat.id.clone()));
                                }
                            }
                            self.seats = s;
                        }
                        Err(e) => {
                            self.seats.clear();
                            if self.fault.is_none() {
                                self.fault = Some(Fault {
                                    kind: errors::classify(&e),
                                    detail: e.to_string(),
                                });
                            }
                        }
                    }
                    match notices {
                        Ok(n) => {
                            self.notices = n.into_iter().map(notice::TimedNotice::new).collect()
                        }
                        // L'état de panne (droits, service arrêté) est déjà affiché en pleine page.
                        Err(e) if self.fault.is_none() => {
                            self.modal_error = Some(notice::service_error(&e.to_string()))
                        }
                        Err(_) => {}
                    }
                }
                Event::Notice(Ok(n)) => {
                    self.notices.retain(|old| old.notice.id != n.id);
                    self.notices.insert(0, notice::TimedNotice::new(n));
                    self.schedule = None;
                    self.modal_error = None;
                }
                Event::Notice(Err(e)) => {
                    self.modal_error = Some(notice::service_error(&e.to_string()))
                }
                Event::PreviewStarted(seat, Err(e)) if self.preview.as_deref() == Some(&seat) => {
                    self.preview_error = Some(notice::service_error(&e.to_string()));
                }
                Event::PreviewFrame(seat, result) if self.preview.as_deref() == Some(&seat) => {
                    match result {
                        Ok(frame) if frame.active => {
                            match notice::image_from_frame(&frame) {
                                Ok(Some(image)) => {
                                    if let Some(texture) = &mut self.preview_texture {
                                        texture.set(image, egui::TextureOptions::LINEAR);
                                    } else {
                                        self.preview_texture = Some(ctx.load_texture(
                                            "aperçu-place",
                                            image,
                                            egui::TextureOptions::LINEAR,
                                        ));
                                    }
                                    self.preview_error = None;
                                }
                                Ok(None) => {}
                                Err(e) => self.preview_error = Some(e),
                            }
                            self.preview_frame = Some(frame);
                        }
                        Ok(_) => self.preview_error = Some("capture arrêtée par le service".into()),
                        Err(e) => self.preview_error = Some(notice::service_error(&e.to_string())),
                    }
                }
                Event::Accounts(result) => {
                    self.accounts_requested = false;
                    match result {
                        Ok(list) => {
                            if self.join_after_create.is_some() && self.join_create_stage.is_none()
                            {
                                if let (Some((request_id, _)), Some((name, _, _, _))) =
                                    (&self.join_after_create, &mut self.new_member)
                                {
                                    let pseudo = self.join.pseudo(request_id).unwrap_or("");
                                    if *name == derive_member_name(pseudo, &[]) {
                                        let existing: Vec<String> =
                                            list.iter().map(|a| a.username.clone()).collect();
                                        *name = derive_member_name(pseudo, &existing);
                                    }
                                }
                            }
                            self.accounts = Some(list);
                            if self.join_create_stage.is_none() {
                                self.members_error = None;
                            }
                        }
                        Err(e) => self.members_error = Some(notice::service_error(&e.to_string())),
                    }
                }
                Event::AccountCreated(result) => {
                    self.seat_pending = false;
                    match result {
                        Ok(_) => {
                            if self.join_create_stage == Some(JoinCreateStage::Member) {
                                self.join_create_stage = Some(JoinCreateStage::Seat);
                                self.new_member = None;
                                if let Some((account, display_name)) = self.add_seat.clone() {
                                    self.seat_pending = true;
                                    self.dispatch(Command::SeatCreate {
                                        account,
                                        display_name,
                                        graphics: self.graphics_choice.clone(),
                                    });
                                }
                            } else {
                                self.new_member = None;
                            }
                            self.members_error = None;
                            self.accounts = None;
                            self.request_accounts();
                        }
                        Err(e) => {
                            self.join_create_stage = None;
                            self.members_error = Some(account_create_error(&e.to_string()));
                        }
                    }
                }
                Event::AccountDeleted(result) => {
                    self.seat_pending = false;
                    match result {
                        Ok(report) => {
                            self.member_delete_retry = self.delete_member.take();
                            self.member_delete_report = Some(report);
                            self.accounts = None;
                            self.request_accounts();
                            self.dispatch(Command::Refresh);
                        }
                        Err(e) => self.modal_error = Some(e.to_string()),
                    }
                }
                Event::GpuBudget(result) => match result {
                    Ok(b) => {
                        self.reserve_mb = b.host_reserve_mb;
                        self.gpu_budget = Some(b);
                        self.gpu_error = None;
                    }
                    Err(e) => self.gpu_error = Some(graphics::error_text(&e.to_string())),
                },
                Event::LighteningReport(id, result) => {
                    if self.seat_settings.as_deref() == Some(&id) {
                        match result {
                            Ok(r) => self.lightening_report = Some(r),
                            Err(e) => {
                                self.seat_settings_status =
                                    Some((false, graphics::error_text(&e.to_string())))
                            }
                        }
                    }
                }
                Event::Library(result) => {
                    self.library.loading = false;
                    match result {
                        Ok((data, options)) => {
                            self.library.options = options;
                            self.library.data = Some(data);
                            self.library.error = None;
                        }
                        Err(e) => self.library.error = Some(graphics::error_text(&e.to_string())),
                    }
                }
                Event::LibraryChanged(key, result) => {
                    self.library.pending.finish(&key);
                    match result {
                        Ok(_) => {
                            if key == library::ADD_KEY {
                                self.library.name.clear();
                                self.library.exe_path.clear();
                            }
                            self.dispatch(Command::LibraryList);
                        }
                        Err(e) => self.library.error = Some(e.to_string()),
                    }
                }
                Event::SeatCreated(result) => {
                    self.seat_pending = false;
                    match result {
                        Ok(seat) => {
                            self.members_error = None;
                            self.page = Page::Seats;
                            self.created_highlight = Some((seat.id.clone(), Instant::now()));
                            if let Some((request_id, graphics)) = self.join_after_create.take() {
                                if self.join.accept_created(&request_id, &seat) {
                                    self.join_create_stage = Some(JoinCreateStage::Accept);
                                    self.seat_pending = true;
                                    self.dispatch(Command::JoinAccept {
                                        request_id,
                                        seat_id: seat.id.clone(),
                                        graphics,
                                    });
                                } else {
                                    self.modal_error = Some("La demande a expiré, le membre et la place sont prêts ; demandez au joueur de réessayer.".into());
                                    self.add_seat = None;
                                    self.join_create_stage = None;
                                }
                            } else {
                                self.add_seat = None;
                                self.join_create_stage = None;
                            }
                            self.new_member = None;
                            self.accounts = None;
                            self.request_accounts();
                            self.dispatch(Command::LogonTasksList);
                        }
                        Err(e) => {
                            if self.join_create_stage == Some(JoinCreateStage::Seat)
                                && self.join_new_member
                            {
                                self.join_create_stage = Some(JoinCreateStage::RetrySeat);
                                self.members_error = Some(format!("Le membre a été créé et conservé, mais la place n'a pas pu être créée : {}", graphics::error_text(&e.to_string())));
                            } else {
                                self.join_create_stage = None;
                                self.members_error = Some(graphics::error_text(&e.to_string()));
                            }
                        }
                    }
                }
                Event::SeatDeleted(result) => {
                    self.seat_pending = false;
                    match result {
                        Ok(report) => {
                            self.delete_retry_id = self.delete_seat.take();
                            self.delete_report = Some(report);
                            self.dispatch(Command::AccountsList);
                        }
                        Err(e) => self.modal_error = Some(notice::seat_error(&e.to_string())),
                    }
                }
                Event::LogonTasks(result) => {
                    self.logon_tasks = match result {
                        Ok(tasks) => {
                            if self.logon_tasks.is_none() {
                                self.logon_selected = tasks
                                    .iter()
                                    .filter(|task| !task.restricted)
                                    .map(logon_tasks::key)
                                    .collect();
                            }
                            self.logon_error = None;
                            Some(tasks)
                        }
                        Err(e) if join::is_unknown_method(&e.to_string()) => {
                            self.logon_details = false;
                            None
                        }
                        Err(e) => {
                            self.logon_error = Some(e.to_string());
                            None
                        }
                    };
                }
                Event::LogonTaskChanged(name, restore, result) => match result {
                    Ok(_) => {
                        self.logon_report.push(format!(
                            "{name} : {}.",
                            if restore {
                                "rétabli à chaque place"
                            } else {
                                "limité à ma session"
                            }
                        ));
                        self.dispatch(Command::LogonTasksList);
                    }
                    Err(e) => {
                        self.logon_report.push(format!("{name} : échec — {e}"));
                        self.logon_error = Some(e.to_string());
                    }
                },
                Event::RefreshRate(result) => {
                    self.refresh_rate_pending = false;
                    match result {
                        Ok(rate) => {
                            self.refresh_rate = Some(rate);
                            self.refresh_rate_error = None;
                        }
                        Err(e) => {
                            self.refresh_rate_error = Some(notice::seat_error(&e.to_string()))
                        }
                    }
                }
                Event::SeatSetting(method, result) => {
                    self.seat_pending = false;
                    self.seat_settings_status = Some(match result {
                        Ok(_) => (
                            true,
                            match method {
                                "seat.repair" => "Réparation effectuée.".into(),
                                "seat.resolution.set" => "Résolution appliquée.".into(),
                                _ => "Réglage enregistré.".into(),
                            },
                        ),
                        Err(e) => (false, notice::seat_error(&e.to_string())),
                    });
                }
                Event::JoinRequests(Ok(list)) => self.join.apply_list(list),
                // Liste illisible : silencieux, la panne du service est déjà signalée ailleurs.
                Event::JoinRequests(Err(_)) => {}
                Event::JoinAnswered(id, accepted, result) => {
                    let error = result.err().map(|e| e.to_string());
                    if accepted && self.join_create_stage == Some(JoinCreateStage::Accept) {
                        self.seat_pending = false;
                        self.add_seat = None;
                        self.join_create_stage = None;
                        if error.as_deref().is_some_and(|e| {
                            e.starts_with("not_found") || e.starts_with("conflict")
                        }) {
                            self.modal_error = Some("La demande a expiré, le membre et la place sont prêts ; demandez au joueur de réessayer.".into());
                        }
                    }
                    self.join.answered(&id, accepted, error.as_deref());
                }
                Event::Clients(seat, Ok(names)) => {
                    self.clients.insert(seat, names);
                    self.devices_error = None;
                }
                Event::Clients(_, Err(e)) => {
                    self.devices_error = Some(notice::service_error(&e.to_string()))
                }
                Event::Pair(Ok(())) => {
                    self.pair_pending = false;
                    self.pairing = false;
                    self.pair_pin.clear();
                    self.devices_error = None;
                }
                Event::Pair(Err(e)) => {
                    self.pair_pending = false;
                    self.devices_error = Some(notice::service_error(&e.to_string()));
                }
                Event::ClientsRemoved(Err(e)) => {
                    self.devices_error = Some(notice::service_error(&e.to_string()))
                }
                Event::ClientsRemoved(Ok(())) => self.devices_error = None,
                Event::Settings(result) => {
                    self.settings_pending = false;
                    match result {
                        Ok(settings) => {
                            self.show_indicator = Some(settings.preview.show_indicator);
                            self.settings_error = None;
                        }
                        Err(e) => self.settings_error = Some(notice::service_error(&e.to_string())),
                    }
                }
                Event::Authorization(result) => {
                    let revoking = self.authorization_pending;
                    self.authorization_pending = false;
                    match result {
                        Ok(state) => {
                            if revoking && !state.authorized {
                                logging::record(
                                    "INFO",
                                    "Autorisation d'ouverture sans UAC retirée pour ce compte",
                                );
                                self.authorization_message = Some((
                                    "Autorisation retirée. La prochaine ouverture de Kinseat demandera de nouveau les droits administrateur.".into(),
                                    false,
                                ));
                            }
                            self.authorization = Some(state);
                        }
                        Err(e) => {
                            self.authorization_message =
                                Some((notice::service_error(&e.to_string()), true))
                        }
                    }
                }
                Event::DiagnosticsLevel(result) => {
                    self.diagnostic_pending = false;
                    match result {
                        Ok(level) => {
                            self.diagnostic_level = Some(level);
                            logging::set_detailed(level == DiagnosticLevel::Detailed);
                            self.diagnostic_error = None;
                        }
                        Err(_) => {
                            self.diagnostic_error = Some(
                                "Impossible de lire ou modifier le niveau des journaux.".into(),
                            )
                        }
                    }
                }
                Event::DiagnosticsExport(result) => {
                    self.export_pending = false;
                    match result {
                        Ok(path) => {
                            self.export_path = Some(path);
                            self.export_error = None;
                            logging::record("INFO", "Rapport de diagnostic créé");
                        }
                        Err(_) => self.export_error = Some("Création du rapport impossible. Vérifiez la connexion au service et les droits administrateur.".into()),
                    }
                }
                Event::Action(seat, action, result) => {
                    self.seat_actions_pending.remove(&seat);
                    if let Err(e) = result {
                        let label = match action.as_str() {
                            "disconnect" => "Déconnecter le flux",
                            "restart_stream" => "Relancer le flux",
                            "close" => "Fermer toute la session",
                            _ => "Action",
                        };
                        self.modal_error = Some(format!(
                            "{label} : {}",
                            notice::service_error(&e.to_string())
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    fn stop_preview(&mut self) {
        if let Some(seat) = self.preview.take() {
            self.dispatch(Command::PreviewStop(seat));
        }
        self.preview_texture = None;
        self.preview_frame = None;
        self.preview_error = None;
    }
    fn open_preview(&mut self, seat: String) {
        self.stop_preview();
        self.preview = Some(seat.clone());
        self.preview_started = Instant::now();
        if !self.demo {
            self.dispatch(Command::PreviewStart(seat));
        }
    }
    fn open_schedule(&mut self, seat: String) {
        self.minutes = "15".into();
        self.message = session::DEFAULT_MESSAGE.into();
        self.modal_error = None;
        self.schedule = Some(seat);
    }
    fn security_panel(&mut self, ui: &mut egui::Ui) {
        theme::frame().inner_margin(24).show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));
            ui.label(theme::title("Sécurité", 19.0));
            ui.add_space(14.0);
            match self.authorization {
                Some(state) if state.authorized => {
                    ui.label(theme::text("Ce compte Windows ouvre Kinseat sans demande d’autorisation.", 14.0, theme::PRIMARY));
                    ui.add_space(6.0);
                    ui.label(theme::text("L’autorisation a été donnée une fois avec les droits administrateur. Elle cesse si le compte n’est plus administrateur du PC.", 13.0, theme::SECONDARY));
                    ui.add_space(12.0);
                    if ui.add_enabled(!self.authorization_pending, egui::Button::new(theme::text("Retirer l’autorisation", 14.0, theme::PRIMARY)).corner_radius(6).min_size(egui::vec2(200.0, 32.0))).clicked() {
                        self.authorization_pending = true;
                        self.authorization_message = None;
                        logging::record("INFO", "Retrait de l'autorisation d'ouverture demandé");
                        self.dispatch(Command::AuthorizationRevoke);
                    }
                }
                Some(state) => {
                    ui.label(theme::text(
                        if state.elevated { "Kinseat est ouvert en administrateur. Ce compte n’est pas autorisé à l’ouvrir sans demande." } else { "Ce compte n’est pas autorisé à ouvrir Kinseat sans demande." },
                        14.0, theme::PRIMARY,
                    ));
                    ui.add_space(6.0);
                    ui.label(theme::text("À la prochaine ouverture, Windows demandera une seule fois les droits administrateur.", 13.0, theme::SECONDARY));
                }
                None => {
                    ui.label(theme::text("Chargement…", 14.0, theme::SECONDARY));
                }
            }
            if let Some((message, error)) = &self.authorization_message {
                ui.add_space(10.0);
                ui.label(theme::text(message, 13.0, if *error { theme::ERR } else { theme::SECONDARY }));
            }
        });
    }
    fn ui_preferences(&self) -> preferences::Preferences {
        preferences::Preferences {
            reduced_motion: self.reduced_motion,
            default_profile: self.default_profile.clone(),
            reserve_mb: self.reserve_mb,
            dismissed_logon_tasks: self.dismissed_logon_tasks.clone(),
        }
    }
    fn save_ui_preferences(&self) {
        if let Err(error) = preferences::store(&self.ui_preferences()) {
            logging::record(
                "WARN",
                &format!("Préférences d'interface non enregistrées : {error}"),
            );
        }
    }
    fn settings_panel(&mut self, ui: &mut egui::Ui) {
        let previous = self.ui_preferences();
        if self.refresh_rate.is_none()
            && !self.refresh_rate_pending
            && self.refresh_rate_error.is_none()
        {
            self.refresh_rate_pending = true;
            self.dispatch(Command::RefreshRateGet);
        }
        theme::frame().inner_margin(24).show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));
            ui.label(theme::title("Diffusion", 19.0));
            ui.add_space(14.0);
            ui.label(theme::text("Images par seconde des places", 14.0, theme::PRIMARY));
            if let Some(rate) = self.refresh_rate.clone() {
                ui.horizontal_wrapped(|ui| {
                    for interval in &rate.allowed_intervals_ms {
                        let hz = kinseat_ipc::interval_hz(*interval);
                        let label = if *interval == rate.default_interval_ms { format!("{hz} i/s (recommandé)") } else { format!("{hz} i/s") };
                        if ui.add_enabled(!self.refresh_rate_pending, egui::Button::selectable(rate.interval_ms == *interval, label)).clicked() && rate.interval_ms != *interval {
                            self.refresh_rate_pending = true;
                            self.dispatch(Command::RefreshRateSet(*interval));
                        }
                    }
                });
                ui.add_space(6.0);
                ui.label(theme::text("Commun à toutes les places. Plus élevé : jeu plus fluide, mais plus de charge sur la carte graphique. Pris en compte au prochain démarrage de chaque place (une place déjà ouverte garde sa fréquence).", 13.0, theme::SECONDARY));
            } else if let Some(error) = &self.refresh_rate_error {
                ui.label(theme::text(error, 14.0, theme::ERR));
            } else {
                ui.label(theme::text("Chargement du réglage…", 14.0, theme::SECONDARY));
            }
            ui.add_space(8.0);
            ui.label(theme::text("La qualité d’image, la résolution et le démarrage automatique se règlent par place : Places → menu de la place → Réglages de la place.", 13.0, theme::SECONDARY));
        });
        ui.add_space(16.0);
        if self.logon_tasks.is_some() {
            theme::frame().show(ui, |ui| {
                ui.label(theme::title("Programmes au démarrage des places", 19.0));
                if ui.button("Détails…").clicked() {
                    self.logon_details = true;
                    self.dispatch(Command::LogonTasksList);
                }
            });
            ui.add_space(16.0);
        }
        theme::frame().inner_margin(24).show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));
            ui.label(theme::title("Graphismes et confort", 19.0));
            ui.label(theme::text("Réserve VRAM pour le poste principal (Mo)", 14.0, theme::PRIMARY));
            ui.add(egui::DragValue::new(&mut self.reserve_mb).range(0..=65536).speed(128));
            if ui.button("Appliquer la réserve").clicked() {
                self.dispatch(Command::GpuReserve(self.reserve_mb));
            }
            if let Some(budget) = &self.gpu_budget {
                ui.label(theme::text(format!("{} Mo utilisables · {} Mo attribués · {} Mo libres. Budget indicatif, pas de réservation matérielle.", budget.usable_mb, budget.allocated_mb, budget.free_mb), 13.0, theme::SECONDARY));
            }
            if let Some(error) = &self.gpu_error { ui.label(theme::text(error, 13.0, theme::ERR)); }
            ui.add_space(10.0);
            ui.label("Profil graphique par défaut des nouvelles places");
            egui::ComboBox::from_id_salt("default-graphics-profile")
                .selected_text(graphics::profile_label(&self.default_profile))
                .show_ui(ui, |ui| {
                    for profile in ["Economy", "Balanced", "Quality"] {
                        ui.selectable_value(&mut self.default_profile, profile.to_owned(), graphics::profile_label(profile));
                    }
                });
            ui.checkbox(&mut self.reduced_motion, "Réduire les animations");
        });
        ui.add_space(16.0);
        theme::frame().inner_margin(24).show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));
            ui.label(theme::title("Confidentialité", 19.0));
            ui.add_space(14.0);
            if let Some(mut enabled) = self.show_indicator {
                let control = ui.add_enabled(
                    !self.settings_pending,
                    egui::Checkbox::new(&mut enabled, "Afficher l'indicateur d'aperçu chez le joueur"),
                );
                if control.changed() {
                    self.settings_pending = true;
                    self.dispatch(Command::SettingsSet(enabled));
                }
            } else {
                ui.label(theme::text("Chargement du réglage…", 14.0, theme::SECONDARY));
            }
            ui.add_space(8.0);
            ui.label(theme::text(
                "Masquer l'indicateur ne désactive pas la capture. Le préavis de fin reste toujours visible.",
                13.0, theme::SECONDARY,
            ));
            if let Some(error) = &self.settings_error {
                ui.add_space(12.0);
                ui.label(theme::text(error, 14.0, theme::ERR));
            }
        });
        ui.add_space(16.0);
        let service_active = self.health.as_ref().map(|h| h.rdp_wrapper_active);
        self.multisession.show(ui, self.demo, service_active);
        ui.add_space(16.0);
        self.security_panel(ui);
        ui.add_space(16.0);
        theme::frame().inner_margin(24).show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));
            ui.label(theme::title("Assistance", 19.0));
            ui.add_space(14.0);
            ui.label(theme::text("Niveau des journaux", 14.0, theme::PRIMARY));
            ui.horizontal(|ui| {
                for (level, label) in [(DiagnosticLevel::Normal, "Normal"), (DiagnosticLevel::Detailed, "Détaillé")] {
                    if ui.add_enabled(!self.diagnostic_pending && self.diagnostic_level.is_some(), egui::Button::selectable(self.diagnostic_level == Some(level), label)).clicked() {
                        self.diagnostic_pending = true;
                        logging::record("INFO", "Modification du niveau des journaux demandée");
                        self.dispatch(Command::DiagnosticsSetLevel(level));
                    }
                }
            });
            if self.diagnostic_level.is_none() && !self.demo {
                ui.label(theme::text("Chargement du niveau…", 13.0, theme::SECONDARY));
            }
            if let Some(error) = &self.diagnostic_error {
                ui.label(theme::text(error, 14.0, theme::ERR));
            }
            ui.add_space(18.0);
            let button = egui::Button::new(theme::title("Créer un rapport de diagnostic", 14.0).color(theme::ON_ACCENT))
                .fill(theme::ACCENT).corner_radius(6).min_size(egui::vec2(260.0, 36.0));
            if ui.add_enabled(!self.export_pending, button).clicked() {
                self.export_pending = true;
                self.export_path = None;
                self.export_error = None;
                logging::record("INFO", "Création d'un rapport de diagnostic demandée");
                self.dispatch(Command::DiagnosticsExport);
            }
            if self.export_pending { ui.label(theme::text("Création du rapport en cours…", 14.0, theme::SECONDARY)); }
            if let Some(path) = &self.export_path {
                ui.label(theme::text(if self.demo { "Rapport fictif (démonstration)" } else { "Rapport créé" }, 14.0, theme::OK));
                ui.label(theme::text(path, 13.0, theme::SECONDARY));
                if ui.add_enabled(!self.demo, egui::Button::new("Ouvrir le dossier")).clicked()
                    && open_report_folder(path).is_err()
                {
                    self.export_error = Some("Impossible d’ouvrir le dossier du rapport.".into());
                }
            }
            if let Some(error) = &self.export_error { ui.label(theme::text(error, 14.0, theme::ERR)); }
            ui.add_space(12.0);
            ui.label(theme::text("Le rapport rassemble les journaux de Kinseat, l'état des places et des informations sur le PC. Les mots de passe, codes PIN et clés sont masqués. Envoyez ce fichier .zip pour signaler un problème.", 13.0, theme::SECONDARY));
        });
        if previous != self.ui_preferences() {
            self.save_ui_preferences();
        }
    }
    fn rail(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("navigation")
            .exact_width(220.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(theme::RAIL).inner_margin(16))
            .show(ctx, |ui| {
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    // Marque : deux places côte à côte, dessinées (pas d'emoji).
                    let (r, _) =
                        ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(
                        egui::Rect::from_min_size(
                            r.min + egui::vec2(0.0, 6.0),
                            egui::vec2(9.0, 14.0),
                        ),
                        3.0,
                        theme::ACCENT,
                    );
                    p.rect_filled(
                        egui::Rect::from_min_size(
                            r.min + egui::vec2(12.0, 2.0),
                            egui::vec2(9.0, 18.0),
                        ),
                        3.0,
                        theme::SECONDARY,
                    );
                    ui.add_space(4.0);
                    ui.label(theme::title("Kinseat", 21.0));
                });
                ui.add_space(32.0);
                for page in [
                    Page::Overview,
                    Page::Seats,
                    Page::Members,
                    Page::Devices,
                    Page::Library,
                ] {
                    self.nav_item(ui, page);
                }
                let remaining = (ui.available_height() - 130.0).max(24.0);
                ui.add_space(remaining);
                self.nav_item(ui, Page::Settings);
                ui.add_space(8.0);
                let (line, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 1.0),
                    egui::Sense::hover(),
                );
                ui.painter().rect_filled(line, 0.0, theme::BORDER);
                ui.add_space(12.0);
                views::badge(
                    ui,
                    if self.demo {
                        "Démonstration"
                    } else if self.fault.is_some() {
                        "Service indisponible"
                    } else if self.pending && self.health.is_none() {
                        "Connexion…"
                    } else {
                        "Service en cours"
                    },
                    if self.fault.is_some() {
                        theme::FAINT
                    } else {
                        theme::OK
                    },
                );
                let machine =
                    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Machine locale".into());
                ui.label(theme::text(machine, 12.0, theme::SECONDARY));
                ui.label(theme::text(
                    format!("Version {}", env!("CARGO_PKG_VERSION")),
                    12.0,
                    theme::FAINT,
                ));
            });
    }
    fn nav_item(&mut self, ui: &mut egui::Ui, page: Page) {
        let selected = self.page == page;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), egui::Sense::click());
        let p = ui.painter();
        if selected || response.hovered() {
            p.rect_filled(rect, 6.0, theme::HOVER);
        }
        if selected {
            // Barre ambre de 3 px : la sélection ne repose pas que sur la couleur du fond.
            p.rect_filled(
                egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 8.0), egui::vec2(3.0, 20.0)),
                1.5,
                theme::ACCENT,
            );
        }
        p.text(
            egui::pos2(rect.left() + 16.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            page.label(),
            if selected {
                theme::title_font(14.0)
            } else {
                egui::FontId::proportional(14.0)
            },
            if selected {
                theme::PRIMARY
            } else {
                theme::SECONDARY
            },
        );
        if response.clicked() {
            if self.page != page {
                self.stop_preview();
                self.page_transition = Instant::now();
            }
            self.page = page;
            if page == Page::Settings && !self.demo {
                self.settings_pending = true;
                self.diagnostic_pending = true;
                self.dispatch(Command::SettingsGet);
                self.dispatch(Command::DiagnosticsGetLevel);
                self.dispatch(Command::AuthorizationGet);
            }
            logging::record("INFO", "Navigation dans l'application");
        }
        ui.add_space(4.0);
    }
    fn show_fault(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if let Some(fault) = &self.fault {
            let (title, explanation) = errors::description(fault.kind);
            let mut retry = false;
            let mut elevate = false;
            ui.add_space(32.0);
            theme::frame().inner_margin(24).show(ui, |ui| {
                ui.set_width(ui.available_width().min(620.0));
                views::badge(
                    ui,
                    if fault.kind == errors::Kind::Permission {
                        "Accès limité"
                    } else {
                        "Hors ligne"
                    },
                    theme::WARN,
                );
                ui.add_space(8.0);
                ui.label(theme::title(title, 21.0));
                ui.add_space(8.0);
                ui.label(theme::text(explanation, 14.0, theme::SECONDARY));
                ui.add_space(24.0);
                let label = if fault.kind == errors::Kind::Permission {
                    "Autoriser Kinseat sur ce compte (une seule fois)"
                } else {
                    "Réessayer"
                };
                let clicked = ui
                    .add(
                        egui::Button::new(theme::title(label, 14.0).color(theme::ON_ACCENT))
                            .fill(theme::ACCENT)
                            .corner_radius(6)
                            .min_size(egui::vec2(160.0, 36.0)),
                    )
                    .clicked();
                if fault.kind == errors::Kind::Permission {
                    elevate = clicked;
                } else {
                    retry = clicked;
                }
                ui.add_space(16.0);
                egui::CollapsingHeader::new(theme::text(
                    "Détails techniques",
                    12.0,
                    theme::SECONDARY,
                ))
                .show(ui, |ui| {
                    ui.label(theme::text(&fault.detail, 12.0, theme::SECONDARY));
                });
            });
            if retry {
                self.refresh(ctx);
            }
            if elevate {
                #[cfg(windows)]
                match errors::elevate() {
                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(error) => {
                        self.screenshot_error =
                            Some(format!("Relance annulée ou impossible : {error}"))
                    }
                }
            }
        }
        if let Some(error) = &self.screenshot_error {
            ui.label(theme::text(error, 14.0, theme::ERR));
        }
    }
    fn preview_panel(&mut self, ctx: &egui::Context) {
        let Some(seat) = self.preview.clone() else {
            return;
        };
        if self.demo {
            let image = notice::demo_image(self.preview_started.elapsed());
            if let Some(texture) = &mut self.preview_texture {
                texture.set(image, egui::TextureOptions::LINEAR);
            } else {
                self.preview_texture =
                    Some(ctx.load_texture("image-démo", image, egui::TextureOptions::LINEAR));
            }
        }
        let mut close = false;
        let mut schedule = false;
        let member = self
            .seats
            .iter()
            .find(|s| s.id == seat)
            .map(|s| s.account_name.as_str())
            .unwrap_or("Membre inconnu");
        let screen = ctx.screen_rect().size();
        egui::Window::new("Aperçu de session").id(egui::Id::new("seat-preview"))
            .collapsible(false).resizable(true)
            .default_size([(screen.x - 80.0).min(740.0), (screen.y - 60.0).min(570.0)])
            .max_size([screen.x - 40.0, screen.y - 40.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(theme::title(format!("{member} · place {seat}"), 17.0));
                    let (label, color) = if self.demo { ("Image de démonstration".into(), theme::WARN) }
                        else if let Some(f) = &self.preview_frame {
                            if f.live && f.age_seconds.unwrap_or(0.0) <= 3.0 {
                                ("En direct".into(), theme::OK)
                            } else {
                                let time = f.captured_at_utc.as_deref().and_then(|s| s.get(11..19)).unwrap_or("--:--:--");
                                (format!("Image du {time} — pas de direct"), theme::WARN)
                            }
                        } else { ("Démarrage de l'aperçu…".into(), theme::WARN) };
                    views::badge(ui, &label, color);
                });
                ui.add_space(12.0);
                if let Some(error) = &self.preview_error { ui.label(theme::text(format!("Aperçu indisponible : {error}"), 14.0, theme::ERR)); }
                if let Some(texture) = &self.preview_texture {
                    let size = texture.size_vec2();
                    let room_h = (ui.ctx().screen_rect().height() - 250.0).max(140.0);
                    let width = ui.available_width().min(700.0).min(room_h * size.x / size.y);
                    ui.image((texture.id(), egui::vec2(width, width * size.y / size.x)));
                } else if self.preview_error.is_none() { ui.label("Démarrage de l'aperçu…"); }
                ui.add_space(12.0);
                ui.label(theme::text(if self.show_indicator == Some(false) { "L’indicateur est masqué chez le joueur. Aucune image n’est enregistrée." } else { "Le joueur voit un badge “Aperçu en cours” pendant l’observation. Aucune image n’est enregistrée." }, 14.0, theme::SECONDARY));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Arrêter l'aperçu").clicked() { close = true; }
                    if ui.add(egui::Button::new(theme::title("Programmer la fin…", 14.0).color(theme::ON_ACCENT)).fill(theme::ACCENT)).clicked() { schedule = true; }
                });
            });
        if close {
            self.stop_preview();
        }
        if schedule {
            self.open_schedule(seat);
        }
    }
    fn seat_settings_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.seat_settings.clone() else {
            return;
        };
        let Some(seat) = self.seats.iter().find(|s| s.id == id).cloned() else {
            self.seat_settings = None;
            return;
        };
        let mut open = true;
        let mut command: Option<(&'static str, serde_json::Value)> = None;
        let busy = self.seat_pending;
        let screen = ctx.screen_rect().size();
        egui::Window::new(format!("Réglages · {}", seat.display_name.as_deref().unwrap_or(&seat.account_name)))
            .id(egui::Id::new("seat-settings"))
            .open(&mut open)
            .collapsible(false).resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .max_height(screen.y - 60.0)
            .show(ctx, |ui| {
                ui.set_width(460.0_f32.min(screen.x - 80.0));
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.label(theme::title("Graphismes", 15.0));
                    let mut graphics = kinseat_ipc::GraphicsChoice::preset(seat.graphics_profile.as_deref().unwrap_or("Balanced"));
                    if graphics.graphics_profile == "Custom" {
                        graphics.vram_budget_mb = Some(seat.vram_budget_mb);
                        graphics.width = Some(seat.width);
                        graphics.height = Some(seat.height);
                        graphics.fps = Some(seat.fps);
                        graphics.nvenc_preset = seat.nvenc_preset.clone();
                    }
                    graphics::selector(ui, "seat-graphics", &mut self.graphics_choice);
                    if ui.add_enabled(!busy, egui::Button::new("Appliquer le profil graphique")).clicked() {
                        let r = kinseat_ipc::graphics_request(70, &id, &self.graphics_choice);
                        command = Some(("seat.graphics.set", r.params));
                    }
                    ui.label(theme::text(format!("Profil actuel : {} · budget indicatif {} Mo · VRAM mesurée : {}", graphics::profile_label(&graphics.graphics_profile), seat.vram_budget_mb, seat.vram_used_mb.map(|v| format!("{v} Mo")).unwrap_or_else(|| "indisponible".into())), 13.0, theme::SECONDARY));
                    if seat.vram_over_budget { ui.label(theme::text("Alerte : utilisation mesurée au-dessus du budget indicatif.", 13.0, theme::WARN)); }
                    ui.add_space(14.0);
                    ui.label(theme::title("Session allégée", 15.0));
                    let light = &mut self.lightening_draft;
                    ui.horizontal(|ui| {
                        if ui.button("Mode agressif").clicked() { *light = kinseat_ipc::Lightening::default(); }
                        if ui.button("Mode modéré").clicked() { *light = kinseat_ipc::Lightening { block_vendor_utilities: false, close_windhawk_tray: false, ..Default::default() }; }
                    });
                    ui.checkbox(&mut light.empty_desktop, "Masquer les raccourcis du bureau");
                    ui.add_enabled(light.empty_desktop, egui::Checkbox::new(&mut self.desktop_browser_draft, "Navigateur sur le bureau"));
                    ui.checkbox(&mut light.block_startup_apps, "Bloquer les applications au démarrage");
                    ui.checkbox(&mut light.block_vendor_utilities, "Fermer les utilitaires constructeur");
                    ui.checkbox(&mut light.close_windhawk_tray, "Fermer l’icône et l’interface Windhawk");
                    ui.label(theme::text("L’injection Windhawk vient d’un service du PC et ne peut pas être désactivée pour une seule session. Seule l’icône et l’interface de cette place sont fermées.", 12.0, theme::SECONDARY));
                    if (*light != seat.lightening || self.desktop_browser_draft != self.desktop_browser.get(&id).copied().unwrap_or(false)) && ui.add_enabled(!busy, egui::Button::new("Appliquer l’allègement")).clicked() {
                        let mut params = serde_json::to_value(&*light).expect("lightening");
                        params["desktopBrowser"] = serde_json::json!(self.desktop_browser_draft);
                        command = Some(("seat.lightening.set", params));
                    }
                    if let Some(report) = &self.lightening_report {
                        ui.label(theme::text(format!("{} processus arrêtés · {} entrées de démarrage bloquées", report.stopped_processes.len(), report.blocked_startup_entries.len()), 13.0, theme::SECONDARY));
                        for process in &report.stopped_processes { ui.label(theme::text(format!("{} · {} · {}", process.name, process.category, process.at), 12.0, theme::FAINT)); }
                    } else { ui.label(theme::text("Rapport en cours de chargement…", 12.0, theme::FAINT)); }
                    ui.add_space(14.0);
                    ui.label(theme::title("Démarrage", 15.0));
                    let mut auto = seat.auto_start;
                    if ui.add_enabled(!busy, egui::Checkbox::new(&mut auto, "Préparer la place au démarrage du PC")).changed() {
                        command = Some(("seat.autostart.set", serde_json::json!({"enabled": auto})));
                    }
                    ui.label(theme::text("Sinon, la place est préparée à la première connexion.", 12.0, theme::SECONDARY));
                    ui.add_space(14.0);
                    ui.label(theme::title("Qualité de l’image", 15.0));
                    let current = seat.nvenc_preset.clone().unwrap_or_else(|| "Balanced".into());
                    ui.horizontal(|ui| {
                        for (value, label) in [("Latency", "Réactivité"), ("Balanced", "Équilibré"), ("Quality", "Qualité")] {
                            if ui.add_enabled(!busy, egui::Button::selectable(current == value, label)).clicked() && current != value {
                                command = Some(("seat.preset.set", serde_json::json!({"preset": value})));
                            }
                        }
                    });
                    ui.label(theme::text("Réactivité : latence minimale, pour les jeux rapides. Qualité : image plus fine, demande plus de débit. Appliqué au prochain lancement du streaming.", 12.0, theme::SECONDARY));
                    ui.add_space(14.0);
                    ui.label(theme::title("Résolution de l’écran virtuel", 15.0));
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("seat-res")
                            .selected_text(format!("{} × {}", self.seat_resolution.0, self.seat_resolution.1))
                            .show_ui(ui, |ui| {
                                for r in [(1280, 720), (1600, 900), (1920, 1080), (2560, 1440), (3840, 2160)] {
                                    ui.selectable_value(&mut self.seat_resolution, r, format!("{} × {}", r.0, r.1));
                                }
                            });
                        let changed = self.seat_resolution != (seat.width, seat.height);
                        if ui.add_enabled(!busy && changed, egui::Button::new("Appliquer")).clicked() {
                            command = Some(("seat.resolution.set", serde_json::json!({"width": self.seat_resolution.0, "height": self.seat_resolution.1})));
                        }
                    });
                    ui.label(theme::text(format!("Actuelle : {} × {}. Le changement est immédiat si l’écran est actif.", seat.width, seat.height), 12.0, theme::SECONDARY));
                    ui.add_space(14.0);
                    ui.label(theme::title("Dépannage", 15.0));
                    ui.horizontal_wrapped(|ui| {
                        for (target, label, hint) in [
                            ("audio", "Réparer le son", "Recrée la sortie audio de la place."),
                            ("display", "Réparer l’écran", "Réinitialise l’écran virtuel de la place."),
                            ("controller", "Réparer la manette", "Recrée la manette virtuelle de la place."),
                        ] {
                            if ui.add_enabled(!busy, egui::Button::new(label)).on_hover_text(hint).clicked() {
                                command = Some(("seat.repair", serde_json::json!({"target": target})));
                            }
                        }
                    });
                    if busy {
                        ui.add_space(8.0);
                        ui.label(theme::text("Opération en cours…", 13.0, theme::SECONDARY));
                    }
                    if let Some((ok, message)) = &self.seat_settings_status {
                        ui.add_space(8.0);
                        ui.label(theme::text(message, 13.0, if *ok { theme::OK } else { theme::ERR }));
                    }
                });
            });
        if let Some((method, params)) = command {
            self.seat_pending = !self.demo;
            self.seat_settings_status = None;
            self.dispatch(Command::SeatSetting {
                seat: id.clone(),
                method,
                params,
            });
            if method == "seat.lightening.set" && !self.demo {
                self.dispatch(Command::LighteningReport(id));
            }
        }
        if !open {
            self.seat_settings = None;
        }
    }
    fn logon_tasks_banner(&mut self, ui: &mut egui::Ui) {
        let Some(tasks) = self.logon_tasks.clone() else {
            return;
        };
        if !logon_tasks::visible(&tasks, &self.dismissed_logon_tasks) {
            return;
        }
        let n = tasks.iter().filter(|task| !task.restricted).count();
        theme::frame().inner_margin(4).show(ui, |ui| {
            // Les actions gardent leur largeur naturelle ; elles passent sous le texte
            // plutôt que de pousser le dernier bouton hors de la fenêtre.
            let stacked = ui.available_width() < 740.0;
            let message = logon_tasks::count_label(n);
            let actions_width = 420.0;
            if stacked {
                ui.add_sized(
                    [ui.available_width(), 20.0],
                    egui::Label::new(theme::text(&message, 12.0, theme::PRIMARY)).truncate(),
                )
                .on_hover_text(&message);
            }
            ui.horizontal(|ui| {
                if !stacked {
                    let label_width = (ui.available_width() - actions_width).max(0.0);
                    ui.add_sized(
                        [label_width, 24.0],
                        egui::Label::new(theme::text(&message, 12.0, theme::PRIMARY)).truncate(),
                    )
                    .on_hover_text(&message);
                } else {
                    ui.add_space((ui.available_width() - actions_width).max(0.0));
                }
                if ui.button("Les limiter à ma session…").clicked() {
                    self.logon_selected = tasks
                        .iter()
                        .filter(|t| !t.restricted)
                        .map(logon_tasks::key)
                        .collect();
                    self.logon_confirm = Some(
                        tasks
                            .iter()
                            .filter(|t| !t.restricted)
                            .map(|t| (t.path.clone(), t.name.clone(), false))
                            .collect(),
                    );
                }
                if ui.link("Détails").clicked() {
                    self.logon_details = true;
                }
                if ui.small_button("Ne plus afficher").clicked() {
                    self.dismissed_logon_tasks = tasks
                        .iter()
                        .filter(|t| !t.restricted)
                        .map(logon_tasks::key)
                        .collect();
                    self.save_ui_preferences();
                }
            });
        });
        ui.add_space(4.0);
    }
    fn request_accounts(&mut self) {
        if self.accounts.is_none() && !self.accounts_requested {
            self.accounts_requested = true;
            self.dispatch(Command::AccountsList);
        }
    }
    fn open_add_seat(&mut self, account: Option<String>) {
        self.members_error = None;
        self.request_accounts();
        self.add_seat = Some((account.unwrap_or_default(), String::new()));
        self.graphics_choice = kinseat_ipc::GraphicsChoice::preset(&self.default_profile);
        if !self.demo {
            self.dispatch(Command::GpuBudget);
        }
    }
    fn seat_modals(&mut self, ctx: &egui::Context) {
        if let Some((mut account, mut display)) = self.add_seat.clone() {
            let (mut cancel, mut submit, mut open_member) = (false, false, false);
            egui::Window::new("Ajouter une place")
                .collapsible(false).resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .max_width(480.0)
                .show(ctx, |ui| {
                    ui.set_width(440.0);
                    ui.label(theme::text("La place ouvre une session Windows dédiée au membre, diffusée vers son appareil.", 14.0, theme::SECONDARY));
                    ui.add_space(12.0);
                    let free: Vec<String> = self.accounts.as_deref().unwrap_or_default().iter()
                        .filter(|a| a.is_managed && !a.has_seat).map(|a| a.username.clone()).collect();
                    let from_join = self.join_after_create.is_some();
                    if from_join {
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut self.join_new_member, true, "Nouveau membre");
                            ui.add_enabled_ui(!free.is_empty() && self.join_create_stage.is_none(), |ui| {
                                ui.selectable_value(&mut self.join_new_member, false, "Membre existant");
                            });
                        });
                        if free.is_empty() { self.join_new_member = true; }
                    }
                    if from_join && self.join_new_member {
                        if let Some((name, mode, pw, pw2)) = self.new_member.as_mut() {
                            ui.label("Nom du compte (lettres, chiffres, tiret)");
                            ui.add_enabled(!self.seat_pending && self.join_create_stage.is_none(), egui::TextEdit::singleline(name).desired_width(330.0).char_limit(20));
                            if self.join_create_stage.is_none() { member_credentials(ui, mode, pw, pw2); }
                        }
                    } else {
                        ui.label("Membre (compte Windows)");
                        if self.accounts.is_none() {
                            ui.label(theme::text("Chargement des comptes…", 13.0, theme::SECONDARY));
                        } else if free.is_empty() {
                            if self.accounts.as_ref().is_some_and(|accounts| !accounts.iter().any(|a| a.is_managed)) {
                                ui.label("Aucun membre pour l'instant.");
                                if ui.button("Ajouter un membre").clicked() {
                                    open_member = true;
                                    cancel = true;
                                }
                            } else {
                                ui.label(theme::text("Tous les membres ont déjà une place. Ajoutez d’abord un membre depuis la page Membres.", 13.0, theme::WARN));
                            }
                        } else {
                            egui::ComboBox::from_id_salt("seat-account")
                                .selected_text(if account.is_empty() { "Choisir un membre" } else { account.as_str() })
                                .show_ui(ui, |ui| {
                                    for name in &free { ui.selectable_value(&mut account, name.clone(), name); }
                                });
                        }
                    }
                    ui.add_space(6.0);
                    ui.label("Nom affiché (facultatif)");
                    ui.add(egui::TextEdit::singleline(&mut display).desired_width(330.0).char_limit(40).hint_text("ex. Salon, Chambre de Léa"));
                    graphics::selector(ui, "create-graphics", &mut self.graphics_choice);
                    graphics::gauge(ui, self.gpu_budget.as_ref(), &self.graphics_choice, self.reduced_motion);
                    if let Some(error) = &self.gpu_error { ui.label(theme::text(error, 13.0, theme::WARN)); }
                    if let Some(error) = &self.members_error { ui.add_space(6.0); ui.label(theme::text(error, 13.0, theme::ERR)); }
                    if self.seat_pending {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(match self.join_create_stage {
                                Some(JoinCreateStage::Member) => "Création du membre…",
                                Some(JoinCreateStage::Accept) => "Acceptation de la demande…",
                                _ => "Préparation de la place…",
                            });
                        });
                    }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!self.seat_pending, egui::Button::new("Annuler")).clicked() { cancel = true; }
                        let new_member = self.join_after_create.is_some() && self.join_new_member;
                        let ready = if self.join_create_stage == Some(JoinCreateStage::RetrySeat) {
                            !account.is_empty()
                        } else if new_member {
                            self.new_member.is_some() && self.accounts.is_some()
                        } else {
                            !account.is_empty() && free.contains(&account)
                        };
                        let label = if self.join_create_stage == Some(JoinCreateStage::RetrySeat) {
                            "Réessayer la création de la place"
                        } else if new_member { "Créer le membre et la place" } else { "Créer la place" };
                        if ui.add_enabled(!self.seat_pending && ready,
                            egui::Button::new(theme::text(label, 14.0, theme::ON_ACCENT)).fill(theme::ACCENT)).clicked() { submit = true; }
                    });
                });
            if cancel && !self.seat_pending {
                self.add_seat = None;
                self.join_after_create = None;
                self.join_create_stage = None;
                self.new_member = if open_member {
                    Some((
                        String::new(),
                        CredentialMode::default(),
                        String::new(),
                        String::new(),
                    ))
                } else {
                    None
                };
                self.members_error = None;
            } else {
                self.add_seat = Some((account.clone(), display.clone()));
            }
            if submit {
                self.members_error = None;
                if self.join_after_create.is_some()
                    && self.join_new_member
                    && self.join_create_stage != Some(JoinCreateStage::RetrySeat)
                {
                    if let Some((name, mode, pw, pw2)) = self.new_member.clone() {
                        if !valid_member_name(&name) {
                            self.members_error = Some("Nom invalide : 1 à 20 caractères, lettres sans accent, chiffres, - ou _.".into());
                        } else if let Err(error) = mode.validate(&pw, &pw2) {
                            self.members_error = Some(error.into());
                        } else if self.accounts.as_ref().is_some_and(|list| {
                            list.iter().any(|a| a.username.eq_ignore_ascii_case(&name))
                        }) {
                            self.members_error = Some("Ce nom de membre est déjà utilisé.".into());
                        } else {
                            self.seat_pending = !self.demo;
                            self.join_create_stage = Some(JoinCreateStage::Member);
                            self.add_seat = Some((name.clone(), display));
                            self.new_member =
                                Some((name.clone(), mode, String::new(), String::new()));
                            self.dispatch(Command::AccountCreate {
                                name,
                                credential: mode.credential(pw),
                            });
                        }
                    }
                } else {
                    self.seat_pending = !self.demo;
                    if self.join_after_create.is_some() {
                        self.join_create_stage = Some(JoinCreateStage::Seat);
                    }
                    self.dispatch(Command::SeatCreate {
                        account,
                        display_name: display.trim().to_owned(),
                        graphics: self.graphics_choice.clone(),
                    });
                }
            }
        }
        if self.new_member.is_some() && self.add_seat.is_none() {
            if let Some((mut name, mut mode, mut pw, mut pw2)) = self.new_member.clone() {
                let (mut cancel, mut submit) = (false, false);
                egui::Window::new("Ajouter un membre")
                .collapsible(false).resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .max_width(480.0)
                .show(ctx, |ui| {
                    ui.set_width(440.0);
                    ui.label(theme::text("Kinseat crée un compte Windows local standard (sans droits d’administration). Choisissez comment le joueur ouvrira sa session.", 14.0, theme::SECONDARY));
                    ui.add_space(12.0);
                    ui.label("Nom du compte (lettres, chiffres, tiret)");
                    ui.add(egui::TextEdit::singleline(&mut name).desired_width(330.0).char_limit(20));
                    let previous_mode = mode;
                    member_credentials(ui, &mut mode, &mut pw, &mut pw2);
                    if mode != previous_mode { self.members_error = None; }
                    if let Some(error) = &self.members_error { ui.add_space(6.0); ui.label(theme::text(error, 13.0, theme::ERR)); }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Annuler").clicked() { cancel = true; }
                        if ui.add_enabled(!self.seat_pending,
                            egui::Button::new(theme::text("Créer le membre", 14.0, theme::ON_ACCENT)).fill(theme::ACCENT)).clicked() { submit = true; }
                    });
                });
                if cancel {
                    self.new_member = None;
                    self.members_error = None;
                } else if submit {
                    let valid_name = !name.is_empty()
                        && name.len() <= 20
                        && name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
                    if !valid_name {
                        self.members_error = Some(
                        "Nom invalide : 1 à 20 caractères, lettres sans accent, chiffres, - ou _."
                            .into(),
                    );
                        self.new_member = Some((name, mode, pw, pw2));
                    } else if let Err(error) = mode.validate(&pw, &pw2) {
                        self.members_error = Some(error.into());
                        if pw != pw2 {
                            pw2.clear();
                        }
                        self.new_member = Some((name, mode, pw, pw2));
                    } else {
                        self.seat_pending = !self.demo;
                        self.members_error = None;
                        self.new_member = Some((name.clone(), mode, String::new(), String::new()));
                        self.dispatch(Command::AccountCreate {
                            name,
                            credential: mode.credential(pw),
                        });
                    }
                } else {
                    self.new_member = Some((name, mode, pw, pw2));
                }
            }
        }
        if let Some(id) = self.delete_seat.clone() {
            let (mut cancel, mut confirm) = (false, false);
            let who = self
                .seats
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.account_name.clone())
                .unwrap_or_default();
            egui::Window::new("Supprimer cette place ?")
                .collapsible(false).resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .max_width(480.0)
                .show(ctx, |ui| {
                    ui.set_width(440.0);
                    ui.label(theme::text(format!("La session Windows de {who} sera fermée : les données non sauvegardées seront perdues."), 14.0, theme::ERR));
                    ui.checkbox(&mut self.purge_account, "Supprimer aussi le compte Windows de ce joueur et ses fichiers");
                    if self.seat_pending { ui.spinner(); }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Annuler").clicked() { cancel = true; }
                        if ui.add_enabled(!self.seat_pending, egui::Button::new(theme::text("Supprimer la place", 14.0, theme::ON_ACCENT)).fill(theme::ERR)).clicked() { confirm = true; }
                    });
                });
            if cancel {
                self.delete_seat = None;
            }
            if confirm {
                self.seat_pending = !self.demo;
                self.dispatch(Command::SeatDelete {
                    seat: id,
                    purge_account: self.purge_account,
                });
            }
        }
        if let Some(report) = &self.delete_report {
            let mut close = false;
            let mut retry = false;
            egui::Window::new("Compte rendu de suppression")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(errors::removed_summary(&report.removed));
                    for failure in &report.failed {
                        ui.label(theme::text(
                            format!(
                                "{} : {}",
                                errors::delete_item_label(&failure.item),
                                failure.reason
                            ),
                            13.0,
                            theme::ERR,
                        ));
                    }
                    if let Some(reason) = &report.account_kept {
                        ui.label(format!(
                            "Compte Windows conservé : {}",
                            errors::account_kept_label(reason)
                        ));
                    }
                    if !report.failed.is_empty()
                        && self.delete_retry_id.is_some()
                        && ui.button("Réessayer").clicked()
                    {
                        retry = true;
                    }
                    if ui.button("Fermer").clicked() {
                        close = true;
                    }
                });
            if retry {
                self.delete_report = None;
                self.delete_seat = self.delete_retry_id.clone();
                if let Some(id) = self.delete_seat.clone() {
                    self.seat_pending = !self.demo;
                    self.dispatch(Command::SeatDelete {
                        seat: id,
                        purge_account: self.purge_account,
                    });
                }
            } else if close {
                self.delete_report = None;
                self.delete_retry_id = None;
            }
        }
        if let Some(account) = self.delete_member.clone() {
            let has_seat = self.accounts.as_ref().is_some_and(|accounts| {
                accounts.iter().any(|a| a.username == account && a.has_seat)
            }) || self.seats.iter().any(|seat| seat.account_name == account);
            let (mut cancel, mut confirm) = (false, false);
            egui::Window::new("Supprimer ce membre ?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .max_width(480.0)
                .show(ctx, |ui| {
                    ui.set_width(440.0);
                    ui.label(theme::text(
                        format!("Le compte Windows « {account} » et tous ses fichiers seront supprimés définitivement."),
                        14.0,
                        theme::ERR,
                    ));
                    if has_seat {
                        ui.label("Sa place sera supprimée aussi.");
                    }
                    ui.checkbox(&mut self.delete_member_checked, "Je comprends");
                    if self.seat_pending {
                        ui.spinner();
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Annuler").clicked() {
                            cancel = true;
                        }
                        if ui.add_enabled(
                            views::member_delete_enabled(self.delete_member_checked, self.seat_pending),
                            egui::Button::new(theme::text("Supprimer le membre", 14.0, theme::ON_ACCENT)).fill(theme::ERR),
                        ).clicked() {
                            confirm = true;
                        }
                    });
                });
            if cancel && !self.seat_pending {
                self.delete_member = None;
                self.delete_member_checked = false;
            } else if confirm {
                self.seat_pending = !self.demo;
                self.dispatch(Command::AccountDelete(account));
            }
        }
        if let Some(report) = &self.member_delete_report {
            let (mut close, mut retry) = (false, false);
            egui::Window::new("Compte rendu de suppression du membre")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(errors::removed_summary(&report.removed));
                    for failure in &report.failed {
                        ui.label(theme::text(
                            format!(
                                "{} : {}",
                                errors::delete_item_label(&failure.item),
                                failure.reason
                            ),
                            13.0,
                            theme::ERR,
                        ));
                    }
                    if let Some(reason) = &report.account_kept {
                        ui.label(format!(
                            "Compte Windows conservé : {}",
                            errors::account_kept_label(reason)
                        ));
                    }
                    if !report.failed.is_empty()
                        && self.member_delete_retry.is_some()
                        && ui.button("Réessayer").clicked()
                    {
                        retry = true;
                    }
                    if ui.button("Fermer").clicked() {
                        close = true;
                    }
                });
            if retry {
                self.member_delete_report = None;
                if let Some(account) = self.member_delete_retry.clone() {
                    self.delete_member = Some(account.clone());
                    self.seat_pending = !self.demo;
                    self.dispatch(Command::AccountDelete(account));
                }
            } else if close {
                self.member_delete_report = None;
                self.member_delete_retry = None;
                self.delete_member_checked = false;
            }
        }
    }
    fn join_modals(&mut self, ctx: &egui::Context) {
        if self.add_seat.is_some()
            && (self.join_after_create.is_some()
                || self.join_create_stage == Some(JoinCreateStage::Accept))
        {
            return;
        }
        let first = self.join.pending().first().map(|r| r.id.clone());
        if !self.demo && first.is_some() && first != self.budget_for_join {
            // Fresh budget for each new request: seats may have changed since the last fetch.
            self.dispatch(Command::GpuBudget);
        }
        self.budget_for_join = first;
        for action in self.join.show(
            ctx,
            &self.seats,
            self.gpu_budget.as_ref(),
            self.reduced_motion,
        ) {
            match action {
                join::Action::Accept {
                    request_id,
                    seat_id,
                    graphics,
                } => {
                    if self.join.accept(&request_id, &seat_id, &self.seats) {
                        self.dispatch(Command::JoinAccept {
                            request_id,
                            seat_id,
                            graphics,
                        });
                    }
                }
                join::Action::CreateSeat {
                    request_id,
                    graphics,
                } => {
                    self.open_add_seat(None);
                    self.graphics_choice = graphics.clone();
                    let existing: Vec<String> = self
                        .accounts
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .map(|a| a.username.clone())
                        .collect();
                    let proposed =
                        derive_member_name(self.join.pseudo(&request_id).unwrap_or(""), &existing);
                    self.join_new_member = self
                        .accounts
                        .as_ref()
                        .is_some_and(|accounts| accounts.iter().all(|a| a.has_seat));
                    self.new_member = Some((
                        proposed,
                        CredentialMode::default(),
                        String::new(),
                        String::new(),
                    ));
                    self.join_create_stage = None;
                    self.join_after_create = Some((request_id, graphics));
                }
                join::Action::Refuse(request_id) => {
                    self.join.refuse(&request_id);
                    self.dispatch(Command::JoinRefuse(request_id));
                }
                join::Action::GoToSeats => self.page = Page::Seats,
                join::Action::Dismiss(request_id) => self.join.dismiss(&request_id),
            }
        }
    }
    fn modals(&mut self, ctx: &egui::Context) {
        if self.logon_details {
            let mut open = true;
            egui::Window::new("Programmes au démarrage des places")
                .open(&mut open)
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .default_width(630.0)
                .max_height(500.0)
                .show(ctx, |ui| {
                    if let Some(mut tasks) = self.logon_tasks.clone() {
                        logon_tasks::sort(&mut tasks);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.add_sized(
                                [55.0, 22.0],
                                egui::Label::new(theme::text("Choix", 13.0, theme::PRIMARY)),
                            );
                            ui.add_sized(
                                [250.0, 22.0],
                                egui::Label::new(theme::text("Programme", 13.0, theme::PRIMARY)),
                            );
                            ui.add_sized(
                                [160.0, 22.0],
                                egui::Label::new(theme::text("État", 13.0, theme::PRIMARY)),
                            );
                            ui.add_sized(
                                [75.0, 22.0],
                                egui::Label::new(theme::text("Action", 13.0, theme::PRIMARY)),
                            );
                        });
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            egui::Grid::new("logon-task-details")
                                .striped(true)
                                .show(ui, |ui| {
                                    for task in &tasks {
                                        let key = logon_tasks::key(task);
                                        let mut selected = self.logon_selected.contains(&key);
                                        if ui
                                            .add_enabled_ui(!task.restricted, |ui| {
                                                ui.add_sized(
                                                    [55.0, 36.0],
                                                    egui::Checkbox::new(&mut selected, ""),
                                                )
                                            })
                                            .inner
                                            .changed()
                                        {
                                            if selected {
                                                self.logon_selected.push(key);
                                            } else {
                                                self.logon_selected.retain(|k| k != &key);
                                            }
                                        }
                                        ui.vertical(|ui| {
                                            ui.add_sized(
                                                [250.0, 17.0],
                                                egui::Label::new(logon_tasks::name(task))
                                                    .truncate(),
                                            );
                                            ui.add_sized(
                                                [250.0, 17.0],
                                                egui::Label::new(theme::text(
                                                    &task.name,
                                                    11.0,
                                                    theme::SECONDARY,
                                                ))
                                                .truncate(),
                                            );
                                        })
                                        .response
                                        .on_hover_text(
                                            format!(
                                                "{}{}\n{}\nCompte : {}",
                                                task.path, task.name, task.execute, task.run_as
                                            ),
                                        );
                                        ui.add_sized(
                                            [160.0, 36.0],
                                            egui::Label::new(if task.restricted {
                                                "Limitée à ma session"
                                            } else {
                                                "À chaque place"
                                            }),
                                        );
                                        if task.restricted && ui.button("Rétablir").clicked() {
                                            self.logon_confirm = Some(vec![(
                                                task.path.clone(),
                                                task.name.clone(),
                                                true,
                                            )]);
                                        } else if !task.restricted {
                                            ui.label("—");
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                        ui.add_space(12.0);
                        if ui.button("Les limiter à ma session…").clicked() {
                            self.logon_confirm = Some(
                                tasks
                                    .iter()
                                    .filter(|t| {
                                        !t.restricted
                                            && self.logon_selected.contains(&logon_tasks::key(t))
                                    })
                                    .map(|t| (t.path.clone(), t.name.clone(), false))
                                    .collect(),
                            );
                        }
                    } else if self.logon_error.is_none() {
                        ui.label("Chargement des programmes…");
                    }
                    if let Some(error) = &self.logon_error {
                        ui.colored_label(theme::ERR, error);
                    }
                    for report in &self.logon_report {
                        ui.label(report);
                    }
                });
            self.logon_details = open;
        }
        if let Some(changes) = self.logon_confirm.clone() {
            let (mut cancel, mut confirm) = (false, false);
            egui::Window::new("Confirmer la modification des tâches Windows")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Ces programmes seront modifiés après confirmation :");
                    for (path, name, restore) in &changes {
                        let key = format!("{path}{name}");
                        let mut selected = self.logon_selected.contains(&key);
                        let display = self
                            .logon_tasks
                            .as_ref()
                            .and_then(|tasks| {
                                tasks.iter().find(|t| t.path == *path && t.name == *name)
                            })
                            .map(logon_tasks::name)
                            .unwrap_or_else(|| name.clone());
                        if *restore {
                            ui.label(format!("Rétablir : {display}"));
                        } else if ui.checkbox(&mut selected, display).changed() {
                            if selected {
                                self.logon_selected.push(key);
                            } else {
                                self.logon_selected.retain(|k| k != &key);
                            }
                        }
                    }
                    if ui.button("Annuler").clicked() {
                        cancel = true;
                    }
                    if ui.button("Confirmer").clicked() {
                        confirm = true;
                    }
                });
            if cancel || confirm {
                self.logon_confirm = None;
            }
            if confirm {
                self.logon_error = None;
                self.logon_report.clear();
                for (path, name, restore) in changes {
                    if restore || self.logon_selected.contains(&format!("{path}{name}")) {
                        self.dispatch(Command::LogonTaskChange {
                            path,
                            name,
                            restore,
                        });
                    }
                }
            }
        }
        self.seat_modals(ctx);
        self.join_modals(ctx);
        self.seat_settings_window(ctx);
        if self.pairing {
            let mut cancel = false;
            let mut submit = false;
            egui::Window::new("Associer un appareil")
                .collapsible(false).resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .max_width(460.0)
                .show(ctx, |ui| {
                    ui.set_width(420.0);
                    ui.label(theme::text("Sur l'appareil, ouvrez Kinseat Client et choisissez cette place : un code s'affiche.", 14.0, theme::SECONDARY));
                    ui.add_space(12.0);
                    ui.label("Place");
                    egui::ComboBox::from_id_salt("pair-place")
                        .selected_text(self.seats.iter().find(|s| s.id == self.pair_seat)
                            .map(|s| s.account_name.as_str()).unwrap_or("Choisir une place"))
                        .show_ui(ui, |ui| {
                            for seat in &self.seats {
                                if seat.status == "Ready" || seat.status == "Streaming" {
                                    ui.selectable_value(&mut self.pair_seat, seat.id.clone(), &seat.account_name);
                                }
                            }
                        });
                    if !self.seats.iter().any(|s| s.status == "Ready" || s.status == "Streaming") {
                        ui.label(theme::text("Aucune place prête.", 13.0, theme::WARN));
                        for seat in &self.seats {
                            ui.label(theme::text(
                                format!("{} : {} — démarrez ou réparez-la depuis Places.",
                                    seat.account_name, views::status(&seat.status).0.to_lowercase()),
                                13.0, theme::SECONDARY));
                        }
                    }
                    ui.label("Nom de l'appareil");
                    ui.add(egui::TextEdit::singleline(&mut self.pair_name).desired_width(330.0).char_limit(40));
                    ui.label("Code affiché sur l'appareil (4 chiffres)");
                    ui.add(egui::TextEdit::singleline(&mut self.pair_pin).font(egui::FontId::monospace(22.0)).desired_width(110.0).char_limit(4));
                    if let Some(error) = &self.devices_error { ui.label(theme::text(error, 13.0, theme::ERR)); }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Annuler").clicked() { cancel = true; }
                        if ui.add_enabled(!self.pair_pending,
                            egui::Button::new(theme::text("Associer", 14.0, theme::ON_ACCENT)).fill(theme::ACCENT)).clicked() { submit = true; }
                    });
                });
            if cancel {
                self.pairing = false;
                self.pair_pin.clear();
                self.devices_error = None;
            }
            if submit {
                if self.pair_seat.is_empty()
                    || self.pair_name.trim().is_empty()
                    || self.pair_name.chars().any(char::is_control)
                    || self.pair_pin.len() != 4
                    || !self.pair_pin.bytes().all(|c| c.is_ascii_digit())
                {
                    self.devices_error =
                        Some("Choisissez une place, un nom et un PIN de 4 chiffres.".into());
                } else {
                    self.pair_pending = true;
                    self.devices_error = None;
                    let command = Command::Pair {
                        seat: self.pair_seat.clone(),
                        pin: std::mem::take(&mut self.pair_pin),
                        name: self.pair_name.trim().to_owned(),
                    };
                    self.dispatch(command);
                }
            }
        }
        if let Some((seat, name)) = self.remove_client.clone() {
            let mut cancel = false;
            let mut confirm = false;
            egui::Window::new("Retirer cet appareil ?")
                .collapsible(false).resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .max_width(460.0).show(ctx, |ui| {
                    ui.set_width(400.0);
                    ui.label(theme::text(format!("{name} ne pourra plus se connecter à cette place sans nouvel appairage."), 14.0, theme::SECONDARY));
                    ui.horizontal(|ui| {
                        if ui.button("Annuler").clicked() { cancel = true; }
                        if ui.add(egui::Button::new("Retirer").fill(theme::ERR)).clicked() { confirm = true; }
                    });
                });
            if cancel || confirm {
                self.remove_client = None;
            }
            if confirm {
                self.dispatch(Command::ClientsRemove { seat, name });
            }
        }
        if let Some(seat) = self.schedule.clone() {
            let member = self
                .seats
                .iter()
                .find(|s| s.id == seat)
                .map(|s| s.account_name.clone())
                .unwrap_or_else(|| "Membre inconnu".into());
            let mut cancel = false;
            let mut submit = false;
            egui::Window::new("Programmer la fin de session").id(egui::Id::new("schedule-modal"))
                .collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_min_width(440.0);
                    ui.label(theme::title(format!("{member} · place {seat}"), 17.0));
                    ui.add_space(16.0);
                    ui.label("Délai avant la fin de session");
                    ui.horizontal(|ui| { for n in [5, 10, 15, 30, 60] { if ui.selectable_label(self.minutes == n.to_string(), format!("{n} min")).clicked() { self.minutes = n.to_string(); } } });
                    ui.horizontal(|ui| { ui.add(egui::TextEdit::singleline(&mut self.minutes).desired_width(64.0)); ui.label("minutes (1 à 120)"); });
                    ui.add_space(12.0);
                    ui.label("Message affiché au joueur");
                    ui.add(egui::TextEdit::multiline(&mut self.message).desired_width(420.0).desired_rows(3));
                    ui.label(theme::text(format!("{} / 220 caractères", self.message.chars().count()), 12.0, theme::SECONDARY));
                    ui.add_space(12.0);
                    ui.label(theme::text("À zéro, le jeu et toutes les applications de la session se fermeront, même sans sauvegarde.", 14.0, theme::WARN));
                    if let Some(error) = &self.modal_error { ui.label(theme::text(error, 14.0, theme::ERR)); }
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if ui.button("Annuler").clicked() { cancel = true; }
                        if ui.add(egui::Button::new(theme::title("Programmer", 14.0).color(theme::ON_ACCENT)).fill(theme::ACCENT)).clicked() { submit = true; }
                    });
                });
            if cancel {
                self.schedule = None;
                self.modal_error = None;
            }
            if submit {
                match notice::validate(&self.minutes, &self.message) {
                    Ok(minutes) => self.dispatch(Command::Schedule {
                        seat,
                        minutes,
                        message: self.message.clone(),
                    }),
                    Err(e) => self.modal_error = Some(e.into()),
                }
            }
        }
        if let Some(seat) = self.confirm_close.clone() {
            let mut close = false;
            let mut confirm = false;
            let mut warn = false;
            let mut disconnect = false;
            egui::Window::new("Fermer la session ?")
                .id(egui::Id::new("confirm-close"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_min_width(420.0);
                    ui.label(theme::text(
                        "Choisissez comment terminer la session du joueur.",
                        14.0,
                        theme::PRIMARY,
                    ));
                    ui.label(theme::text(
                        "Fermer maintenant fait perdre les parties non sauvegardées.",
                        14.0,
                        theme::ERR,
                    ));
                    ui.add_space(12.0);
                    let busy = self.seat_actions_pending.contains_key(&seat);
                    if busy {
                        ui.add(egui::Spinner::new());
                        ui.label("En cours…");
                    }
                    if ui.button("Prévenir puis fermer").clicked() {
                        warn = true;
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Fermer maintenant"))
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Seulement déconnecter le flux"))
                        .clicked()
                    {
                        disconnect = true;
                    }
                    ui.add_space(8.0);
                    if ui.button("Annuler").clicked() {
                        close = true;
                    }
                });
            if close || confirm || warn || disconnect {
                self.confirm_close = None;
            }
            if warn {
                self.open_schedule(seat.clone());
            }
            if disconnect {
                self.dispatch(Command::Action {
                    seat: seat.clone(),
                    action: "disconnect",
                });
            }
            if confirm {
                self.dispatch(Command::Action {
                    seat,
                    action: "close",
                });
            }
        }
        if self.schedule.is_none() && self.confirm_close.is_none() {
            if let Some(error) = self.modal_error.clone() {
                egui::Window::new("Action impossible")
                    .collapsible(false)
                    .show(ctx, |ui| {
                        ui.label(theme::text(error, 14.0, theme::ERR));
                        if ui.button("Fermer").clicked() {
                            self.modal_error = None;
                        }
                    });
            }
        }
    }
    fn capture(&mut self, ctx: &egui::Context) {
        if let Some(path) = &self.screenshot {
            for event in ctx.input(|input| input.events.clone()) {
                if let egui::Event::Screenshot { image, .. } = event {
                    let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
                    for pixel in &image.pixels {
                        bytes.extend_from_slice(&pixel.to_array());
                    }
                    match image::save_buffer(
                        path,
                        &bytes,
                        image.size[0] as u32,
                        image.size[1] as u32,
                        image::ColorType::Rgba8,
                    ) {
                        Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => {
                            self.screenshot_error = Some(format!("Capture impossible : {e}"));
                            self.screenshot = None;
                        }
                    }
                    return;
                }
            }
            self.screenshot_frames = self.screenshot_frames.saturating_add(1);
            if !self.screenshot_requested
                && self.screenshot_frames >= 2
                && !self.pending
                && (self.health.is_some() || self.fault.is_some())
                && (!self.capture_preview
                    || self.preview_texture.is_some()
                    || self.preview_error.is_some()
                    || self.preview_started.elapsed() > Duration::from_secs(20))
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                self.screenshot_requested = true;
            }
        }
    }
}
impl eframe::App for Kinseat {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.read_events(ctx);
        if self.demo && !self.demo_commands.is_empty() {
            let now = Instant::now();
            let mut remaining = Vec::new();
            for (deadline, command) in std::mem::take(&mut self.demo_commands) {
                if deadline <= now {
                    self.demo_command(command);
                } else {
                    remaining.push((deadline, command));
                }
            }
            self.demo_commands.extend(remaining);
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if self.library.pending.any() || self.seat_pending || !self.seat_actions_pending.is_empty()
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if self.capture_preview && self.preview.is_none() {
            if let Some(id) = self
                .seats
                .iter()
                .find(|s| s.session_id > 0)
                .map(|s| s.id.clone())
            {
                self.page = Page::Seats;
                self.open_preview(id);
            }
        }
        if !self.reduced_motion && self.page_transition.elapsed() < Duration::from_millis(240) {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if self.demo && self.preview.is_some()
            || self.notices.iter().any(notice::TimedNotice::active)
        {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        self.rail(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::CANVAS).inner_margin(24))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(theme::title(self.page.label(), 21.0));
                    if self.demo {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            egui::Frame::new()
                                .fill(theme::SURFACE)
                                .corner_radius(6)
                                .inner_margin(egui::Margin::symmetric(10, 6))
                                .show(ui, |ui| {
                                    ui.label(theme::text(
                                        "Données de démonstration",
                                        12.0,
                                        theme::ACCENT,
                                    ));
                                });
                        });
                    }
                });
                ui.add_space(24.0);
                if !self.reduced_motion {
                    let progress = (self.page_transition.elapsed().as_secs_f32() / 0.24).min(1.0);
                    if progress < 1.0 {
                        ui.add_space(12.0 * (1.0 - progress));
                    }
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.fault.is_some() && self.page != Page::Settings {
                            self.show_fault(ui, ctx);
                            return;
                        }
                        if matches!(self.page, Page::Overview | Page::Settings) {
                            self.logon_tasks_banner(ui);
                        }
                        if matches!(self.page, Page::Overview | Page::Seats) && !self.seats.is_empty() && self.health.as_ref().is_some_and(|h| !h.rdp_wrapper_active) {
                            theme::frame().show(ui, |ui| {
                                ui.label(theme::text("Plusieurs sessions Windows ne sont pas actives : les places ne pourront pas démarrer.", 14.0, theme::WARN));
                                if ui.button("Activer plusieurs sessions").clicked() { self.page = Page::Settings; }
                            });
                            ui.add_space(12.0);
                        }
                        match self.page {
                            Page::Overview => {
                                if let Some(health) = &self.health {
                                    if let Some(intent) = overview::show(
                                        ui,
                                        health,
                                        &self.seats,
                                        &self.notices,
                                        self.reduced_motion,
                                    ) {
                                        match intent {
                                            overview::Intent::Preview(id) => self.open_preview(id),
                                            overview::Intent::ConfirmClose(id) => {
                                                self.confirm_close = Some(id)
                                            }
                                            overview::Intent::Seats => self.page = Page::Seats,
                                            overview::Intent::Cancel(id) => {
                                                self.dispatch(Command::Cancel(id))
                                            }
                                        }
                                    }
                                } else {
                                    ui.label("Chargement de la machine…");
                                }
                            }
                            Page::Seats | Page::Preview | Page::Notice => {
                                if self.pending && self.health.is_none() {
                                    ui.label("Chargement des places…");
                                } else if let Some(intent) = views::seats(
                                    ui,
                                    &self.seats,
                                    &self.notices,
                                    self.reduced_motion,
                                    self.created_highlight.as_ref().and_then(|(id, at)| {
                                        (at.elapsed().as_secs() < 3).then_some(id.as_str())
                                    }),
                                    &self.seat_actions_pending,
                                ) {
                                    match intent {
                                        views::SeatIntent::Add => self.open_add_seat(None),
                                        views::SeatIntent::Delete(id) => {
                                            self.purge_account = self
                                                .seats
                                                .iter()
                                                .find(|s| s.id == id)
                                                .and_then(|s| {
                                                    self.accounts.as_ref().and_then(|accounts| {
                                                        accounts
                                                            .iter()
                                                            .find(|a| a.username == s.account_name)
                                                    })
                                                })
                                                .is_some_and(|account| account.is_managed);
                                            self.delete_seat = Some(id);
                                            self.delete_report = None;
                                        }
                                        views::SeatIntent::Settings(id) => {
                                            if let Some(s) = self.seats.iter().find(|s| s.id == id)
                                            {
                                                self.seat_resolution = (s.width, s.height);
                                            }
                                            self.seat_settings_status = None;
                                            self.lightening_report = None;
                                            if let Some(s) = self.seats.iter().find(|s| s.id == id)
                                            {
                                                self.graphics_choice =
                                                    kinseat_ipc::GraphicsChoice::preset(
                                                        s.graphics_profile
                                                            .as_deref()
                                                            .unwrap_or("Balanced"),
                                                    );
                                                if self.graphics_choice.graphics_profile == "Custom"
                                                {
                                                    self.graphics_choice.vram_budget_mb =
                                                        Some(s.vram_budget_mb);
                                                    self.graphics_choice.width = Some(s.width);
                                                    self.graphics_choice.height = Some(s.height);
                                                    self.graphics_choice.fps = Some(s.fps);
                                                    self.graphics_choice.nvenc_preset =
                                                        s.nvenc_preset.clone();
                                                }
                                                self.lightening_draft = s.lightening.clone();
                                            self.desktop_browser_draft = self.desktop_browser.get(&id).copied().unwrap_or(false);
                                            }
                                            self.seat_settings = Some(id.clone());
                                            if !self.demo {
                                                self.dispatch(Command::LighteningReport(id));
                                            }
                                        }
                                        views::SeatIntent::Preview(id) => self.open_preview(id),
                                        views::SeatIntent::Schedule(id) => self.open_schedule(id),
                                        views::SeatIntent::ConfirmClose(id) => {
                                            self.confirm_close = Some(id)
                                        }
                                        views::SeatIntent::Command(cmd) => self.dispatch(cmd),
                                    }
                                }
                            }
                            Page::Members => {
                                self.request_accounts();
                                let error = self.members_error.clone();
                                match views::members(ui, self.accounts.as_deref(), error.as_deref())
                                {
                                    Some(views::MemberIntent::Refresh) => {
                                        self.accounts = None;
                                        self.members_error = None;
                                        self.request_accounts();
                                    }
                                    Some(views::MemberIntent::Create) => {
                                        self.members_error = None;
                                        self.new_member = Some((
                                            String::new(),
                                            CredentialMode::default(),
                                            String::new(),
                                            String::new(),
                                        ));
                                    }
                                    Some(views::MemberIntent::AddSeat(name)) => {
                                        self.open_add_seat(Some(name))
                                    }
                                    Some(views::MemberIntent::Delete(name)) => {
                                        self.delete_member = Some(name);
                                        self.delete_member_checked = false;
                                        self.modal_error = None;
                                    }
                                    None => {}
                                }
                            }
                            Page::Devices => {
                                if let Some(intent) = views::devices(
                                    ui,
                                    &self.seats,
                                    &self.clients,
                                    self.demo,
                                    self.devices_error.as_deref(),
                                ) {
                                    match intent {
                                        views::DeviceIntent::Associate => {
                                            self.pairing = true;
                                            self.pair_seat = self
                                                .seats
                                                .iter()
                                                .find(|s| {
                                                    s.status == "Ready" || s.status == "Streaming"
                                                })
                                                .map(|s| s.id.clone())
                                                .unwrap_or_default();
                                            self.pair_pin.clear();
                                            self.devices_error = None;
                                        }
                                        views::DeviceIntent::Remove(seat, name) => {
                                            self.remove_client = Some((seat, name))
                                        }
                                    }
                                }
                            }
                            Page::Library => {
                                self.request_accounts();
                                if self.library.data.is_none()
                                    && !self.library.loading
                                    && self.library.error.is_none()
                                    && !self.demo
                                {
                                    self.library.loading = true;
                                    self.dispatch(Command::LibraryList);
                                }
                                if let Some(command) = self.library.show(
                                    ui,
                                    self.accounts.as_deref(),
                                    self.reduced_motion,
                                ) {
                                    self.dispatch(command);
                                }
                            }
                            Page::Settings => {
                                if self.show_indicator.is_none()
                                    && !self.settings_pending
                                    && !self.demo
                                {
                                    self.settings_pending = true;
                                    self.dispatch(Command::SettingsGet);
                                }
                                self.settings_panel(ui);
                            }
                        }
                    });
            });
        self.preview_panel(ctx);
        self.modals(ctx);
        self.capture(ctx);
    }
}

fn open_report_folder(path: &str) -> io::Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer.exe")
            .arg(format!("/select,{path}"))
            .spawn()?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(io::Error::other("Explorateur Windows indisponible"))
    }
}

struct Launch {
    demo: bool,
    page: Page,
    screenshot: Option<PathBuf>,
    size: [f32; 2],
    zoom: f32,
    search: String,
    #[cfg_attr(not(windows), allow(dead_code))]
    authorize: bool,
    demo_join: bool,
    logon_tasks: bool,
}
fn options() -> Result<Launch, String> {
    let mut args = std::env::args().skip(1);
    let mut demo = !cfg!(windows);
    let mut page = Page::Overview;
    let mut screenshot = None;
    let mut size = [1280.0, 800.0];
    let mut zoom = 1.0;
    let mut search = String::new();
    let mut authorize = false;
    let mut demo_join = false;
    let mut logon_tasks = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--logon-tasks" => logon_tasks = true,
            "--demo" => demo = true,
            "--authorize" => authorize = true,
            // Capture de la fenêtre « Demande d'accès » avec une demande fictive (implique --demo).
            "--demo-join" => {
                demo = true;
                demo_join = true;
            }
            "--page" => {
                page = Page::parse(&args.next().ok_or("Page manquante")?).ok_or("Page inconnue")?
            }
            "--screenshot" => {
                screenshot = Some(PathBuf::from(args.next().ok_or("Chemin PNG manquant")?))
            }
            "--size" => {
                let v = args.next().ok_or("Taille manquante (ex. 1280x800)")?;
                let (w, h) = v.split_once('x').ok_or("Taille attendue LxH")?;
                size = [
                    w.parse().map_err(|_| "Largeur invalide")?,
                    h.parse().map_err(|_| "Hauteur invalide")?,
                ];
            }
            "--zoom" => {
                zoom = args
                    .next()
                    .ok_or("Zoom manquant")?
                    .parse::<f32>()
                    .map_err(|_| "Zoom invalide")?
                    .clamp(0.5, 3.0)
            }
            // Pré-remplit la recherche de la Bibliothèque (captures de qualification).
            "--search" => search = args.next().ok_or("Texte de recherche manquant")?,
            _ => return Err(format!("Option inconnue : {arg}")),
        }
    }
    Ok(Launch {
        demo,
        page,
        screenshot,
        size,
        zoom,
        search,
        authorize,
        demo_join,
        logon_tasks,
    })
}

/// Première ouverture sur un compte : une seule demande UAC, puis ouverture sans élévation.
/// Renvoie true si cette instance (non élevée) doit se fermer au profit de l'instance élevée.
#[cfg(windows)]
fn first_launch_authorization(authorize: bool) -> bool {
    let state = kinseat_ipc::app_authorization();
    let elevated = errors::is_elevated();
    match state {
        // Instance élevée (relance --authorize ou « Exécuter en tant qu'administrateur ») :
        // l'UAC vient d'être acceptée, on enregistre ce compte.
        Ok(s) if elevated && !s.authorized && (authorize || s.elevated) => {
            match kinseat_ipc::app_authorize() {
                Ok(_) => logging::record(
                    "INFO",
                    "Compte autorisé : Kinseat s'ouvrira désormais sans demande d'autorisation",
                ),
                Err(e) => logging::record("WARN", &format!("Autorisation du compte refusée : {e}")),
            }
            false
        }
        // Pas d'autorisation : une seule relance élevée, jamais depuis une instance déjà élevée.
        Err(e) if kinseat_ipc::is_forbidden(&e) && !elevated && !authorize => {
            logging::record(
                "INFO",
                "Première ouverture sur ce compte : demande des droits administrateur",
            );
            match errors::elevate() {
                Ok(()) => true,
                Err(e) => {
                    logging::record("WARN", &format!("Demande d'autorisation annulée : {e}"));
                    false
                }
            }
        }
        _ => false,
    }
}
fn main() -> eframe::Result {
    logging::init();
    logging::record(
        "INFO",
        concat!("Démarrage de Kinseat version ", env!("CARGO_PKG_VERSION")),
    );
    let launch = options().map_err(|e| {
        eprintln!("{e}");
        eframe::Error::AppCreation(Box::new(io::Error::other(e)))
    })?;
    #[cfg(windows)]
    if !launch.demo && launch.screenshot.is_none() && first_launch_authorization(launch.authorize) {
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(launch.size)
            .with_min_inner_size([740.0, 600.0])
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/kinseat-256.png"))
                    .unwrap_or_default(),
            ),
        ..Default::default()
    };
    eframe::run_native(
        "Kinseat",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_zoom_factor(launch.zoom);
            let mut app = Kinseat::new(
                cc,
                launch.page,
                launch.demo,
                launch.screenshot.clone(),
                launch.logon_tasks,
                launch.search.clone(),
            );
            // Captures de recette : KINSEAT_DEMO_DIALOG=add-seat|new-member|delete-seat (démo uniquement).
            if launch.demo_join {
                app.join.demo_request();
            }
            // Recette sur un vrai service : seul le dialogue « Ajouter une place » (sans effet tant
            // qu'on ne valide pas) peut être ouvert d'office.
            match std::env::var("KINSEAT_DEMO_DIALOG").as_deref() {
                Ok("add-seat") if !launch.demo => app.open_add_seat(None),
                Ok("preview") if !launch.demo => app.capture_preview = true,
                _ => {}
            }
            if launch.demo {
                match std::env::var("KINSEAT_DEMO_DIALOG").as_deref() {
                    Ok("add-seat") => {
                        app.open_add_seat(None);
                        app.graphics_choice = kinseat_ipc::GraphicsChoice {
                            graphics_profile: "Custom".into(),
                            vram_budget_mb: Some(4096),
                            width: Some(2560),
                            height: Some(1440),
                            fps: Some(60),
                            nvenc_preset: Some("Quality".into()),
                        };
                    }
                    Ok("logon-tasks-1") | Ok("logon-tasks") => {
                        app.page = Page::Overview;
                        app.logon_tasks = Some(demo::logon_tasks().into_iter().take(1).collect());
                        app.dismissed_logon_tasks.clear();
                    }
                    Ok("logon-tasks-6") | Ok("logon-tasks-details") => {
                        app.page = Page::Overview;
                        app.logon_tasks = Some(demo::logon_tasks());
                        app.dismissed_logon_tasks.clear();
                        app.logon_details = matches!(
                            std::env::var("KINSEAT_DEMO_DIALOG").as_deref(),
                            Ok("logon-tasks-details")
                        );
                    }
                    Ok("delete-member") => {
                        app.page = Page::Members;
                        app.dispatch(Command::AccountsList);
                        app.delete_member = Some("Camille".into());
                    }
                    Ok("library-access-denied") => {
                        app.page = Page::Library;
                        if let Some(game) =
                            app.library.data.as_mut().and_then(|d| d.games.get_mut(1))
                        {
                            game.access.insert("Yanis".into(), "denied".into());
                        }
                        app.dispatch(Command::AccountsList);
                    }
                    Ok("new-member") => {
                        app.new_member = Some((
                            "lea".into(),
                            CredentialMode::Pin,
                            String::new(),
                            String::new(),
                        ))
                    }
                    Ok("delete-seat") => {
                        app.delete_seat = app.seats.first().map(|s| s.id.clone());
                        app.purge_account = true;
                    }
                    Ok("seat-settings") => {
                        app.seat_settings = app.seats.first().map(|s| s.id.clone())
                    }
                    _ => {}
                }
            }
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod credential_tests {
    use super::{account_create_error, derive_member_name, CredentialMode};

    #[test]
    fn derive_member_name_handles_accents_spaces_length_collision_and_empty() {
        assert_eq!(derive_member_name("Hélène", &[]), "Helene");
        assert_eq!(derive_member_name(" Jean Pierre ", &[]), "JeanPierre");
        assert_eq!(
            derive_member_name("abcdefghijklmnopqrstuvw", &[]),
            "abcdefghijklmnopqrst"
        );
        assert_eq!(derive_member_name("漢字 ✨", &[]), "joueur");
        assert_eq!(derive_member_name("", &["JOUEUR".into()]), "joueur2");
        assert_eq!(
            derive_member_name("Hélène", &["helene".into(), "HELENE2".into()]),
            "Helene3"
        );
        assert_eq!(
            derive_member_name("abcdefghijklmnopqrstuvw", &["abcdefghijklmnopqrst".into()]),
            "abcdefghijklmnopqrs2"
        );
    }

    #[test]
    fn pin_validation() {
        assert_eq!(CredentialMode::default(), CredentialMode::Pin);
        assert!(CredentialMode::Pin.validate("0123", "0123").is_ok());
        for invalid in ["", "123", "12345", "12a4", "１２３４"] {
            assert!(CredentialMode::Pin.validate(invalid, invalid).is_err());
        }
        assert!(CredentialMode::Pin.validate("1234", "4321").is_err());
    }

    #[test]
    fn password_and_no_password_validation() {
        assert!(CredentialMode::Password
            .validate("1234567", "1234567")
            .is_err());
        assert!(CredentialMode::Password
            .validate("12345678", "87654321")
            .is_err());
        assert!(CredentialMode::Password
            .validate("12345678", "12345678")
            .is_ok());
        assert!(CredentialMode::None.validate("", "").is_ok());
        assert!(CredentialMode::None.validate("old", "different").is_ok());
    }

    #[test]
    fn password_policy_error_is_specific() {
        assert_eq!(account_create_error("password_policy: rejected"), "La stratégie de mot de passe de ce PC exige un mot de passe plus long. Choisissez « Mot de passe » ou « Sans mot de passe ».");
        assert_eq!(
            account_create_error("not_found: missing"),
            crate::notice::seat_error("not_found: missing")
        );
    }
}
