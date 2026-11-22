#![cfg_attr(windows, windows_subsystem = "windows")]
mod device;
mod discovery;
mod engine;
mod join;
mod profile;
mod real;
mod recommend;
mod settings;
mod stream_menu;
mod stream_panel;
#[cfg(windows)]
mod stream_windows;
mod theme;

use device::Device;
use egui::{pos2, vec2, Align2, Color32, Key, RichText, Sense, Stroke, StrokeKind};
use recommend::{recommend, same_ratio_options, Decoder};
use settings::Settings;

/// Raccourcis clavier de Moonlight Qt 6.2 pendant un flux.
const STREAM_SHORTCUTS: [(&str, &str); 6] = [
    ("Ctrl + Alt + Maj + Q", "Quitter le flux"),
    (
        "Ctrl + Alt + Maj + Z",
        "Libérer / capturer la souris et le clavier",
    ),
    ("Ctrl + Alt + Maj + X", "Plein écran / fenêtré"),
    ("Ctrl + Alt + Maj + S", "Afficher les statistiques"),
    ("Ctrl + Alt + Maj + D", "Réduire la fenêtre"),
    ("Ctrl + Alt + Maj + V", "Coller le texte du presse-papiers"),
];
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use theme as t;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Search,
    Pair,
    Members,
    Pin,
    Home,
    Stream,
    Settings,
    Profile,
    Waiting,
    Lock,
}
impl Screen {
    fn from_number(n: u8) -> Option<Self> {
        Some(match n {
            1 => Self::Search,
            2 => Self::Pair,
            3 => Self::Members,
            4 => Self::Pin,
            5 => Self::Home,
            6 => Self::Stream,
            7..=9 => Self::Settings,
            10 => Self::Profile,
            11 => Self::Waiting,
            12 => Self::Lock,
            _ => return None,
        })
    }
    fn previous(self) -> Self {
        match self {
            Self::Search => Self::Search,
            Self::Pair => Self::Search,
            Self::Members => Self::Pair,
            Self::Pin => Self::Members,
            Self::Home => Self::Members,
            Self::Stream => Self::Home,
            Self::Settings => Self::Home,
            Self::Profile => Self::Profile,
            Self::Waiting => Self::Search,
            Self::Lock => Self::Lock,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Session {
    Available,
    Paused,
    Busy,
}
const MEMBERS: [&str; 3] = ["Yanis", "Lina", "Adam"];
const CATEGORIES: [&str; 8] = [
    "Vidéo",
    "Affichage",
    "Audio",
    "Entrées",
    "Raccourcis",
    "Session",
    "Appareil",
    "Compte & PC",
];
struct Client {
    screen: Screen,
    return_to: Screen,
    category: usize,
    settings: Settings,
    device: Device,
    screenshot: Option<PathBuf>,
    frames: u8,
    requested: bool,
    started: Instant,
    discovered: bool,
    code: String,
    error: bool,
    member: usize,
    attempts: u8,
    session: Session,
    menu: bool,
    connect_started: Option<Instant>,
    fullscreen: bool,
    notice: Option<String>,
    demo: bool,
    places: Vec<discovery::Place>,
    found: std::sync::mpsc::Receiver<discovery::Place>,
    events_tx: std::sync::mpsc::Sender<engine::Event>,
    events_rx: std::sync::mpsc::Receiver<engine::Event>,
    selected: Option<discovery::Place>,
    address: String,
    join: join::JoinUi,
    form: profile::Form,
    after_lock: Screen,
    playing: bool,
    launching: bool,
    stop_tx: Option<std::sync::mpsc::Sender<()>>,
    panel: stream_menu::Panel,
    panel_tx: std::sync::mpsc::Sender<stream_menu::Action>,
    panel_rx: std::sync::mpsc::Receiver<stream_menu::Action>,
    moonlight_pid: Option<u32>,
    stream_started: Option<Instant>,
    latency_rx: Option<std::sync::mpsc::Receiver<Option<u32>>>,
    latency_samples: std::collections::VecDeque<u32>,
    probe_stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    #[cfg(windows)]
    hook: Option<stream_windows::ShortcutHook>,
    #[cfg(windows)]
    shortcut_rx: Option<std::sync::mpsc::Receiver<()>>,
    manual_entry: bool,
}
fn text(ui: &mut egui::Ui, label: impl Into<String>, size: f32, color: Color32) {
    ui.label(RichText::new(label.into()).size(size).color(color));
}
fn title(ui: &mut egui::Ui, s: &str) {
    ui.label(RichText::new(s).size(32.0).color(t::PRIMARY).strong());
}
fn section(ui: &mut egui::Ui, s: &str) {
    ui.label(RichText::new(s).size(20.0).color(t::PRIMARY).strong());
}
fn space(ui: &mut egui::Ui, n: f32) {
    ui.add_space(n);
}
fn card(ui: &mut egui::Ui, f: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(t::SURFACE)
        .corner_radius(10)
        .inner_margin(24)
        .show(ui, |ui| {
            ui.vertical(f);
        });
}
fn fixed_card(ui: &mut egui::Ui, width: f32, f: impl FnOnce(&mut egui::Ui)) {
    ui.vertical(|ui| {
        ui.set_max_width(width);
        card(ui, f);
    });
}
fn action(ui: &mut egui::Ui, s: &str, primary: bool) -> bool {
    let color = if primary { t::ON_ACCENT } else { t::PRIMARY };
    ui.add_sized(
        [190.0, 42.0],
        egui::Button::new(RichText::new(s).size(15.0).color(color))
            .fill(if primary { t::ACCENT } else { t::HOVER })
            .corner_radius(6),
    )
    .clicked()
}
fn avatar(ui: &mut egui::Ui, member: usize, diameter: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(diameter + 8.0, diameter + 8.0), Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    p.circle_stroke(c, diameter / 2.0 + 3.0, Stroke::new(2.0_f32, t::ACCENT));
    t::draw_avatar(p, c, diameter / 2.0, t::MEMBERS[member]);
}
fn dots(ui: &mut egui::Ui, code: &str, count: usize, error: bool) {
    ui.horizontal(|ui| {
        for i in 0..count {
            let (r, _) = ui.allocate_exact_size(vec2(48.0, 56.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(r, 6, t::HOVER);
            p.rect_stroke(
                r,
                6,
                Stroke::new(
                    1.0_f32,
                    if error {
                        t::ERR
                    } else if i == code.len() {
                        t::ACCENT
                    } else {
                        t::BORDER
                    },
                ),
                StrokeKind::Inside,
            );
            if i < code.len() {
                p.circle_filled(r.center(), 5.0, t::PRIMARY);
            }
        }
    });
}
/// Panneau « radar » de la recherche (démonstration et mode réel) : le point vert n'apparaît
/// qu'une fois un PC trouvé.
fn radar(ui: &mut egui::Ui, col: f32, found: bool) {
    fixed_card(ui, col, |ui| {
        ui.set_width(col - 50.0);
        let (r, _) = ui.allocate_exact_size(vec2(col - 50.0, 300.0), Sense::hover());
        let p = ui.painter();
        let c = r.center();
        for radius in [55.0, 96.0, 135.0] {
            p.circle_stroke(c, radius, Stroke::new(1.0_f32, t::BORDER));
        }
        p.rect_stroke(
            egui::Rect::from_center_size(c, vec2(72.0, 52.0)),
            5,
            Stroke::new(3.0_f32, t::ACCENT),
            StrokeKind::Inside,
        );
        p.line_segment(
            [pos2(c.x - 20.0, c.y + 32.0), pos2(c.x + 20.0, c.y + 32.0)],
            Stroke::new(2.0_f32, t::ACCENT),
        );
        if found {
            p.circle_filled(c + vec2(96.0, -12.0), 5.0, t::OK);
        }
    });
}
/// En-tête commun : logo, mention facultative, bouton Paramètres/Retour. Renvoie true au clic.
fn header(ctx: &egui::Context, settings_open: bool, tag: Option<&str>) -> bool {
    let mut clicked = false;
    egui::TopBottomPanel::top("header")
        .exact_height(68.0)
        .frame(egui::Frame::new().fill(t::RAIL))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.add_space(38.0);
                ui.label(
                    RichText::new("Kinseat")
                        .size(21.0)
                        .color(t::ACCENT)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(34.0);
                    clicked = ui
                        .add_sized(
                            [116.0, 38.0],
                            egui::Button::new(if settings_open {
                                "Retour"
                            } else {
                                "Paramètres"
                            })
                            .fill(t::HOVER),
                        )
                        .clicked();
                    if let Some(tag) = tag {
                        text(ui, tag, 12.0, t::FAINT);
                    }
                });
            });
        });
    clicked
}
fn footer(ctx: &egui::Context) {
    egui::TopBottomPanel::bottom("footer")
        .exact_height(34.0)
        .frame(egui::Frame::new().fill(t::RAIL))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(32.0);
                text(
                    ui,
                    "Échap : retour    ·    F11 : plein écran    ·    Clavier et souris",
                    12.0,
                    t::FAINT,
                );
            });
        });
}
fn picker(ui: &mut egui::Ui, caption: &str, value: &mut String, options: &[&str]) {
    ui.horizontal(|ui| {
        ui.add_sized([210.0, 27.0], egui::Label::new(caption));
        egui::ComboBox::from_id_salt(caption)
            .selected_text(value.as_str())
            .width(250.0)
            .show_ui(ui, |ui| {
                for option in options {
                    ui.selectable_value(value, (*option).to_owned(), *option);
                }
            });
    });
}
fn toggle(ui: &mut egui::Ui, caption: &str, value: &mut bool) {
    ui.checkbox(value, caption);
}
impl Client {
    fn new(
        screen: Screen,
        category: usize,
        screenshot: Option<PathBuf>,
        session: Session,
        demo: bool,
        ctx: &egui::Context,
    ) -> Self {
        t::install(ctx);
        let (found_tx, found) = std::sync::mpsc::channel();
        if !demo {
            discovery::browse(found_tx);
        }
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        let (panel_tx, panel_rx) = std::sync::mpsc::channel();
        let settings = Settings::load();
        let selected = if demo {
            None
        } else {
            settings.linked_place.clone()
        };
        let mut screen = if selected.is_some() {
            Screen::Home
        } else {
            screen
        };
        let after_lock = screen;
        if !demo {
            if settings.pseudo.is_empty() {
                screen = Screen::Profile;
            } else if profile::has_pin(&settings) {
                screen = Screen::Lock;
            }
        }
        let mut join = join::JoinUi::default();
        let mut form = profile::Form {
            pseudo: settings.pseudo.clone(),
            ..Default::default()
        };
        if demo {
            match screen {
                Screen::Profile => form.pseudo = "Yanis".into(),
                Screen::Waiting => {
                    join.phase = Some(join::Phase::Waiting);
                    join.host = Some(join::HostInfo {
                        name: "KINSEAT-HOST".into(),
                        address: "192.168.1.20".into(),
                        version: env!("CARGO_PKG_VERSION").into(),
                    });
                }
                _ => {}
            }
        }
        Self {
            demo,
            found,
            events_tx,
            events_rx,
            selected,
            places: Vec::new(),
            manual_entry: false,
            address: String::new(),
            join,
            form,
            after_lock,
            playing: false,
            launching: false,
            stop_tx: None,
            panel: stream_menu::Panel::default(),
            panel_tx,
            panel_rx,
            moonlight_pid: None,
            stream_started: None,
            latency_rx: None,
            latency_samples: std::collections::VecDeque::new(),
            probe_stop: None,
            #[cfg(windows)]
            hook: None,
            #[cfg(windows)]
            shortcut_rx: None,
            screen,
            return_to: Screen::Home,
            category,
            settings,
            device: Device::detect(ctx),
            discovered: screenshot.is_some(),
            screenshot,
            frames: 0,
            requested: false,
            started: Instant::now(),
            code: String::new(),
            error: false,
            member: 0,
            attempts: 3,
            session,
            menu: false,
            connect_started: None,
            fullscreen: false,
            notice: None,
        }
    }
    fn navigate(&mut self, screen: Screen) {
        self.screen = screen;
        self.code.clear();
        self.error = false;
        self.notice = None;
    }
    fn digit(&mut self, d: char) {
        if self.attempts > 0 && d.is_ascii_digit() && self.code.len() < 6 {
            self.code.push(d);
            self.error = false;
        }
    }
    fn submit(&mut self) {
        match self.screen {
            Screen::Pair if self.code.len() == 6 => {
                if self.code == "482731" {
                    self.navigate(Screen::Members)
                } else {
                    self.code.clear();
                    self.error = true;
                }
            }
            Screen::Pin if (4..=6).contains(&self.code.len()) && self.attempts > 0 => {
                if self.code == "1234" {
                    self.navigate(Screen::Home)
                } else {
                    self.attempts -= 1;
                    self.code.clear();
                    self.error = true;
                }
            }
            Screen::Search if self.discovered => self.navigate(Screen::Pair),
            Screen::Members => self.navigate(Screen::Pin),
            Screen::Home if self.session != Session::Busy => {
                self.navigate(Screen::Stream);
                self.connect_started = Some(Instant::now());
            }
            Screen::Stream => self.menu = !self.menu,
            _ => {}
        }
    }
    fn keyboard(&mut self, ctx: &egui::Context) {
        if !self.demo {
            if ctx.input(|i| i.key_pressed(Key::Escape)) && self.screen == Screen::Settings {
                self.screen = self.return_to;
            }
            return;
        }
        let events = ctx.input(|i| i.events.clone());
        for event in events {
            match event {
                egui::Event::Text(s)
                    if s.len() == 1 && matches!(self.screen, Screen::Pin | Screen::Pair) =>
                {
                    if let Some(c) = s.chars().next() {
                        self.digit(c);
                    }
                }
                egui::Event::Key {
                    key: Key::Backspace,
                    pressed: true,
                    ..
                } if matches!(self.screen, Screen::Pin | Screen::Pair) => {
                    self.code.pop();
                }
                egui::Event::Key {
                    key: Key::Enter,
                    pressed: true,
                    ..
                } => self.submit(),
                egui::Event::Key {
                    key: Key::Escape,
                    pressed: true,
                    ..
                } => {
                    if self.menu {
                        self.menu = false
                    } else if self.screen == Screen::Settings {
                        self.screen = self.return_to
                    } else {
                        self.navigate(self.screen.previous())
                    }
                }
                egui::Event::Key {
                    key: Key::ArrowLeft,
                    pressed: true,
                    ..
                } if self.screen == Screen::Members => self.member = (self.member + 2) % 3,
                egui::Event::Key {
                    key: Key::ArrowRight,
                    pressed: true,
                    ..
                } if self.screen == Screen::Members => self.member = (self.member + 1) % 3,
                egui::Event::Key {
                    key: Key::F11,
                    pressed: true,
                    ..
                } => {
                    self.fullscreen = !self.fullscreen;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
                }
                egui::Event::Key {
                    key: Key::M,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.ctrl
                    && modifiers.alt
                    && modifiers.shift
                    && self.screen == Screen::Stream =>
                {
                    self.menu = !self.menu
                }
                _ => {}
            }
        }
    }
    fn persist(&mut self) {
        if let Err(e) = self.settings.save() {
            self.notice = Some(format!("Paramètres non enregistrés : {e}"));
        }
    }
    fn pad(&mut self, ui: &mut egui::Ui) {
        for row in [
            ["1", "2", "3"],
            ["4", "5", "6"],
            ["7", "8", "9"],
            ["Effacer", "0", "OK"],
        ] {
            ui.horizontal(|ui| {
                for d in row {
                    if ui
                        .add_sized(
                            [76.0, 34.0],
                            egui::Button::new(RichText::new(d).size(15.0).color(if d == "OK" {
                                t::ON_ACCENT
                            } else {
                                t::PRIMARY
                            }))
                            .fill(if d == "OK" {
                                t::ACCENT
                            } else {
                                t::HOVER
                            }),
                        )
                        .clicked()
                    {
                        match d {
                            "Effacer" => {
                                self.code.pop();
                            }
                            "OK" => self.submit(),
                            _ => self.digit(d.chars().next().unwrap()),
                        }
                    }
                }
            });
        }
    }
    fn search(&mut self, ui: &mut egui::Ui) {
        text(ui, "01  /  TROUVER LE PC", 12.0, t::ACCENT);
        space(ui, 12.0);
        title(ui, "Votre PC Kinseat");
        text(
            ui,
            "Retrouvez votre salon sur le réseau local.",
            15.0,
            t::SECONDARY,
        );
        space(ui, 24.0);
        let col = ((ui.available_width() - 24.0) / 2.0).clamp(300.0, 430.0);
        ui.horizontal(|ui| {
            radar(ui, col, self.discovered);
            ui.add_space(24.0);
            ui.vertical(|ui| {
                ui.set_width(col);
                section(ui, "PC disponibles");
                space(ui, 16.0);
                card(ui, |ui| {
                    ui.set_width(col - 50.0);
                    if self.discovered {
                        section(ui, "PC du salon");
                        text(ui, "192.168.1.20  ·  Réseau local", 14.0, t::SECONDARY);
                        space(ui, 12.0);
                        text(ui, "Disponible pour la liaison", 14.0, t::OK);
                        space(ui, 24.0);
                        if action(ui, "Choisir ce PC", true) {
                            self.submit();
                        }
                    } else {
                        section(ui, "Recherche en cours");
                        text(ui, "Recherche sur le réseau local…", 14.0, t::SECONDARY);
                    }
                });
                space(ui, 20.0);
                if action(ui, "Saisir une adresse", false) {
                    self.notice = Some("Saisie indisponible en démonstration".into());
                }
            });
        });
    }
    fn pair(&mut self, ui: &mut egui::Ui) {
        text(ui, "02  /  LIER CET APPAREIL", 12.0, t::ACCENT);
        space(ui, 12.0);
        title(ui, "Lier cet appareil");
        text(
            ui,
            "Un code suffit pour retrouver les profils du foyer.",
            15.0,
            t::SECONDARY,
        );
        space(ui, 20.0);
        let right = 420.0_f32
            .min((ui.available_width() - 20.0) * 0.5)
            .max(400.0);
        let left = (ui.available_width() - 20.0 - right).clamp(360.0, 500.0);
        ui.horizontal(|ui| {
            fixed_card(ui, left, |ui| {
                ui.set_width(left - 50.0);
                let (r, _) = ui.allocate_exact_size(vec2(left - 50.0, 192.0), Sense::hover());
                let p = ui.painter();
                let pc_w = (r.width() - 150.0).clamp(160.0, 240.0);
                let pc = egui::Rect::from_min_size(r.min + vec2(20.0, 26.0), vec2(pc_w, 132.0));
                p.rect_filled(pc, 8, t::HOVER);
                p.rect_stroke(pc, 8, Stroke::new(2.0_f32, t::BORDER), StrokeKind::Inside);
                p.text(
                    pc.center(),
                    Align2::CENTER_CENTER,
                    "4 8 2 7 3 1",
                    egui::FontId::proportional(24.0),
                    t::ACCENT,
                );
                p.line_segment(
                    [
                        pc.center_bottom() + vec2(0.0, 2.0),
                        pc.center_bottom() + vec2(0.0, 18.0),
                    ],
                    Stroke::new(2.0_f32, t::SECONDARY),
                );
                let phone =
                    egui::Rect::from_min_size(pc.right_top() + vec2(40.0, 39.0), vec2(70.0, 100.0));
                p.rect_stroke(
                    phone,
                    8,
                    Stroke::new(3.0_f32, t::ACCENT),
                    StrokeKind::Inside,
                );
                p.line_segment(
                    [
                        pc.right_center() + vec2(4.0, 0.0),
                        phone.left_center() - vec2(4.0, 0.0),
                    ],
                    Stroke::new(1.0_f32, t::ACCENT),
                );
                section(ui, "En trois étapes");
                for (i, s) in [
                    "Sur le PC hôte, ouvrez Appareils.",
                    "Lisez le code affiché à l'écran.",
                    "Saisissez-le ici pour lier ce client.",
                ]
                .iter()
                .enumerate()
                {
                    space(ui, 8.0);
                    text(ui, format!("0{}   {s}", i + 1), 15.0, t::SECONDARY);
                }
            });
            ui.add_space(20.0);
            fixed_card(ui, right, |ui| {
                ui.set_width(right - 50.0);
                section(ui, "Code de liaison");
                space(ui, 12.0);
                dots(ui, &self.code, 6, self.error);
                space(ui, 8.0);
                text(
                    ui,
                    if self.error {
                        "Code incorrect. Vérifiez le PC."
                    } else {
                        "6 chiffres affichés sur le PC du salon"
                    },
                    13.0,
                    if self.error { t::ERR } else { t::SECONDARY },
                );
                space(ui, 18.0);
                self.pad(ui);
                space(ui, 8.0);
                text(ui, "Démo : 482731", 12.0, t::FAINT);
            });
        });
    }
    fn members(&mut self, ui: &mut egui::Ui) {
        text(ui, "03  /  CHOISIR UN JOUEUR", 12.0, t::ACCENT);
        space(ui, 12.0);
        title(ui, "Qui joue ?");
        text(
            ui,
            "Choisissez votre profil pour retrouver votre place.",
            15.0,
            t::SECONDARY,
        );
        space(ui, 30.0);
        let gap = ui.spacing().item_spacing.x;
        let card_w = ((ui.available_width() - gap * (MEMBERS.len() as f32 - 1.0))
            / MEMBERS.len() as f32)
            .clamp(200.0, 280.0);
        ui.horizontal(|ui| {
            for (i, name) in MEMBERS.iter().enumerate() {
                let (r, response) = ui.allocate_exact_size(vec2(card_w, 300.0), Sense::click());
                if response.clicked() {
                    self.member = i;
                    self.attempts = 3;
                    self.navigate(Screen::Pin);
                }
                let p = ui.painter();
                p.rect_filled(
                    r,
                    10,
                    if i == self.member {
                        t::HOVER
                    } else {
                        t::SURFACE
                    },
                );
                if i == self.member {
                    p.rect_stroke(r, 10, Stroke::new(2.0_f32, t::ACCENT), StrokeKind::Inside);
                }
                let c = r.center() + vec2(0.0, -50.0);
                t::draw_avatar(p, c, 58.0, t::MEMBERS[i]);
                p.text(
                    r.center() + vec2(0.0, 53.0),
                    Align2::CENTER_CENTER,
                    *name,
                    egui::FontId::proportional(23.0),
                    t::PRIMARY,
                );
                p.text(
                    r.center() + vec2(0.0, 95.0),
                    Align2::CENTER_CENTER,
                    if i == self.member {
                        "Sélectionné"
                    } else {
                        "Choisir"
                    },
                    egui::FontId::proportional(14.0),
                    if i == self.member {
                        t::ACCENT
                    } else {
                        t::SECONDARY
                    },
                );
                ui.add_space(16.0);
            }
        });
        space(ui, 26.0);
        text(ui, "Profils gérés par le PC hôte", 14.0, t::FAINT);
    }
    fn pin(&mut self, ui: &mut egui::Ui) {
        text(ui, "04  /  IDENTITÉ", 12.0, t::ACCENT);
        space(ui, 12.0);
        title(ui, "Votre espace personnel");
        space(ui, 18.0);
        ui.horizontal(|ui| {
            ui.add_space((ui.available_width() - 460.0).max(0.0) / 2.0);
            card(ui, |ui| {
                ui.set_width(410.0);
                ui.vertical_centered(|ui| {
                    avatar(ui, self.member, 96.0);
                    space(ui, 8.0);
                    section(ui, MEMBERS[self.member]);
                    text(ui, "Saisissez votre code PIN", 14.0, t::SECONDARY);
                    space(ui, 16.0);
                    dots(ui, &self.code, 6, self.error);
                    space(ui, 8.0);
                    text(
                        ui,
                        if self.error {
                            "Code incorrect. Réessayez."
                        } else {
                            "4 à 6 chiffres"
                        },
                        13.0,
                        if self.error { t::ERR } else { t::SECONDARY },
                    );
                    space(ui, 10.0);
                    self.pad(ui);
                    space(ui, 8.0);
                    text(
                        ui,
                        format!("{} essais restants  ·  Démo : 1234", self.attempts),
                        13.0,
                        t::SECONDARY,
                    );
                    space(ui, 8.0);
                    if ui.link("Changer de joueur").clicked() {
                        self.navigate(Screen::Members);
                    }
                });
            });
        });
    }
    fn home(&mut self, ui: &mut egui::Ui) {
        text(ui, "05  /  VOTRE ESPACE", 12.0, t::ACCENT);
        space(ui, 12.0);
        ui.horizontal(|ui| {
            avatar(ui, self.member, 56.0);
            ui.vertical(|ui| {
                title(ui, &format!("Bonjour {}", MEMBERS[self.member]));
                text(ui, "PC du salon  ·  connecté", 14.0, t::SECONDARY);
            });
        });
        space(ui, 18.0);
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let left = (ui.available_width() - 170.0).clamp(220.0, 590.0);
                ui.vertical(|ui| {
                    ui.set_width(left);
                    section(ui, "Votre session");
                    space(ui, 8.0);
                    let (status, color) = match self.session {
                        Session::Available => ("Disponible pour jouer", t::OK),
                        Session::Paused => ("En pause · reprendre", t::ACCENT),
                        Session::Busy => ("PC occupé · aucune place libre", t::ERR),
                    };
                    text(ui, status, 20.0, color);
                    space(ui, 18.0);
                    if self.session != Session::Busy
                        && action(
                            ui,
                            if self.session == Session::Paused {
                                "Reprendre"
                            } else {
                                "Jouer"
                            },
                            true,
                        )
                    {
                        self.submit();
                    }
                });
                let (r, _) = ui.allocate_exact_size(vec2(145.0, 140.0), Sense::hover());
                let p = ui.painter();
                p.circle_stroke(r.center(), 53.0, Stroke::new(9.0_f32, t::HOVER));
                p.circle_stroke(r.center(), 53.0, Stroke::new(5.0_f32, t::ACCENT));
                p.text(
                    r.center() - vec2(0.0, 8.0),
                    Align2::CENTER_CENTER,
                    "1 h 45",
                    egui::FontId::proportional(26.0),
                    t::PRIMARY,
                );
                p.text(
                    r.center() + vec2(0.0, 20.0),
                    Align2::CENTER_CENTER,
                    "restant aujourd’hui",
                    egui::FontId::proportional(11.0),
                    t::SECONDARY,
                );
            });
        });
        space(ui, 16.0);
        let half = ((ui.available_width() - 16.0) / 2.0).clamp(300.0, 520.0);
        ui.horizontal(|ui| {
            fixed_card(ui, half, |ui| {
                ui.set_width(half - 50.0);
                section(ui, "Écran & réseau");
                space(ui, 12.0);
                text(ui, self.device.screen_label(), 14.0, t::PRIMARY);
                let decoder = match self.settings.codec.as_str() {
                    "AV1" => Decoder::Av1,
                    "HEVC" => Decoder::Hevc,
                    "H.264" => Decoder::H264,
                    _ => Decoder::Unknown,
                };
                let profile = recommend(self.device.screen, None, None, decoder);
                text(
                    ui,
                    format!(
                        "Conseillé : {} × {} · {} ips",
                        profile.width, profile.height, profile.fps
                    ),
                    14.0,
                    t::OK,
                );
                text(
                    ui,
                    "Qualité estimée · aucun test réalisé",
                    13.0,
                    t::SECONDARY,
                );
                space(ui, 16.0);
                if action(ui, "Tester ma connexion", false) {
                    self.notice = Some("Test simulé : aucune mesure réelle disponible".into());
                }
            });
            ui.add_space(16.0);
            fixed_card(ui, half, |ui| {
                ui.set_width(half - 50.0);
                section(ui, "Dernière activité");
                space(ui, 12.0);
                text(ui, "Rocket League   ·   Hier", 16.0, t::PRIMARY);
                text(ui, "Minecraft   ·   Cette semaine", 16.0, t::SECONDARY);
                space(ui, 16.0);
                text(ui, "Historique de démonstration", 12.0, t::FAINT);
            });
        });
    }
    fn stream(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.set_min_size(vec2(
                ui.available_width() - 48.0,
                (ui.available_height() - 62.0).max(420.0),
            ));
            ui.horizontal(|ui| {
                text(ui, "PC DU SALON  /  SESSION SIMULÉE", 12.0, t::SECONDARY);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if action(ui, "Menu", false) {
                        self.menu = !self.menu;
                    }
                });
            });
            space(ui, 100.0);
            ui.vertical_centered(|ui| {
                title(ui, "Votre jeu apparaîtra ici");
                text(
                    ui,
                    "Flux vidéo simulé · aucun streaming actif",
                    15.0,
                    t::SECONDARY,
                );
            });
            if self.menu {
                space(ui, 32.0);
                ui.vertical_centered(|ui| {
                    for label in ["Reprendre", "Qualité réseau", "Mettre en pause", "Quitter"] {
                        if action(ui, label, false) {
                            match label {
                                "Reprendre" => self.menu = false,
                                "Qualité réseau" => {
                                    self.notice =
                                        Some("Qualité non mesurée en démonstration".into())
                                }
                                "Mettre en pause" => {
                                    self.session = Session::Paused;
                                    self.navigate(Screen::Home);
                                }
                                _ => {
                                    self.session = Session::Available;
                                    self.navigate(Screen::Home);
                                }
                            }
                        }
                    }
                });
            }
        });
    }
    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        text(ui, "PARAMÈTRES  /  CLIENT", 12.0, t::ACCENT);
        space(ui, 10.0);
        title(ui, "Paramètres");
        space(ui, 14.0);
        let mut modified = false;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(204.0);
                for (i, name) in CATEGORIES.iter().enumerate() {
                    let (r, resp) = ui.allocate_exact_size(vec2(194.0, 36.0), Sense::click());
                    let selected = i == self.category;
                    let p = ui.painter();
                    if selected || resp.hovered() {
                        p.rect_filled(r, 6, t::HOVER);
                    }
                    if selected {
                        p.rect_filled(
                            egui::Rect::from_min_size(
                                r.left_top() + vec2(0.0, 9.0),
                                vec2(3.0, 18.0),
                            ),
                            2,
                            t::ACCENT,
                        );
                    }
                    p.text(
                        r.left_center() + vec2(16.0, 0.0),
                        Align2::LEFT_CENTER,
                        *name,
                        egui::FontId::proportional(14.0),
                        if selected { t::PRIMARY } else { t::SECONDARY },
                    );
                    if resp.clicked() {
                        self.category = i;
                    }
                    ui.add_space(2.0);
                }
            });
            ui.add_space(16.0);
            egui::ScrollArea::vertical()
                .id_salt("settings")
                .max_height((ui.ctx().screen_rect().height() - 215.0).max(300.0))
                .show(ui, |ui| {
                    card(ui, |ui| {
                        // Garde une marge à droite : la carte occupe la place restante, 650 px au plus.
                        ui.set_width((ui.available_width() - 48.0).clamp(420.0, 650.0));
                        section(ui, CATEGORIES[self.category]);
                        space(ui, 15.0);
                        let before = self.settings.clone();
                        match self.category {
                            0 => {
                                let native = format!(
                                    "Automatique · {} × {} (écran natif)",
                                    self.device.screen.width, self.device.screen.height
                                );
                                text(ui, native, 14.0, t::SECONDARY);
                                let opts = same_ratio_options(self.device.screen);
                                let mut choices =
                                    vec!["Automatique".to_owned(), "Personnalisée".to_owned()];
                                choices.extend(opts.iter().map(|(w, h)| format!("{w} × {h}")));
                                picker(
                                    ui,
                                    "Résolution",
                                    &mut self.settings.resolution,
                                    &choices.iter().map(String::as_str).collect::<Vec<_>>(),
                                );
                                if self.settings.resolution == "Personnalisée" {
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::DragValue::new(&mut self.settings.custom_width)
                                                .range(640..=7680),
                                        );
                                        ui.add(
                                            egui::DragValue::new(&mut self.settings.custom_height)
                                                .range(360..=4320),
                                        );
                                    });
                                }
                                picker(
                                    ui,
                                    "Images/s",
                                    &mut self.settings.fps,
                                    &[
                                        "Automatique",
                                        "30",
                                        "60",
                                        "90",
                                        "120",
                                        "144",
                                        "Fréquence écran",
                                    ],
                                );
                                toggle(
                                    ui,
                                    "Débit automatique",
                                    &mut self.settings.automatic_bitrate,
                                );
                                if !self.settings.automatic_bitrate {
                                    ui.add(
                                        egui::Slider::new(&mut self.settings.bitrate, 1..=150)
                                            .text("Débit Mb/s"),
                                    );
                                }
                                picker(
                                    ui,
                                    "Codec",
                                    &mut self.settings.codec,
                                    &["Automatique", "H.264", "HEVC", "AV1"],
                                );
                                picker(
                                    ui,
                                    "Décodeur",
                                    &mut self.settings.decoder,
                                    &["Automatique", "Matériel", "Logiciel"],
                                );
                                for (name, value) in [
                                    ("HDR", &mut self.settings.hdr),
                                    ("YUV 4:4:4", &mut self.settings.yuv444),
                                    ("V-Sync", &mut self.settings.vsync),
                                    ("Régulation des images", &mut self.settings.frame_pacing),
                                ] {
                                    toggle(ui, name, value);
                                }
                            }
                            1 => {
                                picker(
                                    ui,
                                    "Mode",
                                    &mut self.settings.display_mode,
                                    &["Plein écran", "Fenêtré sans bordure", "Fenêtré"],
                                );
                                toggle(
                                    ui,
                                    "Garder l’écran allumé pendant le jeu",
                                    &mut self.settings.keep_awake,
                                );
                            }
                            2 => {
                                picker(
                                    ui,
                                    "Canaux",
                                    &mut self.settings.audio,
                                    &["Stéréo", "5.1", "7.1"],
                                );
                                toggle(
                                    ui,
                                    "Lire aussi le son sur le PC hôte",
                                    &mut self.settings.play_on_host,
                                );
                                text(
                                    ui,
                                    "Désactivé : le son n’est entendu que sur cet appareil.",
                                    13.0,
                                    t::SECONDARY,
                                );
                            }
                            3 => {
                                picker(
                                    ui,
                                    "Immersion clavier",
                                    &mut self.settings.keyboard_capture,
                                    &["Toujours", "En plein écran seulement", "Jamais"],
                                );
                                for (name, value) in [
                                    (
                                        "Optimiser la souris pour le bureau à distance",
                                        &mut self.settings.remote_mouse,
                                    ),
                                    (
                                        "Inverser les boutons de souris",
                                        &mut self.settings.invert_mouse,
                                    ),
                                    ("Inverser le défilement", &mut self.settings.invert_scroll),
                                    (
                                        "Manette : inverser A/B et X/Y",
                                        &mut self.settings.swap_gamepad,
                                    ),
                                    ("Plusieurs manettes", &mut self.settings.multiple_gamepads),
                                    (
                                        "Entrées manette en arrière-plan",
                                        &mut self.settings.background_gamepad,
                                    ),
                                ] {
                                    toggle(ui, name, value);
                                }
                            }
                            4 => {
                                text(
                                    ui,
                                    "Raccourcis disponibles pendant le jeu (fixés par le moteur de streaming) :",
                                    14.0,
                                    t::SECONDARY,
                                );
                                space(ui, 6.0);
                                for (keys, name) in STREAM_SHORTCUTS {
                                    ui.horizontal(|ui| {
                                        ui.add_sized([230.0, 28.0], egui::Label::new(egui::RichText::new(keys).size(14.0).color(t::PRIMARY)));
                                        ui.label(egui::RichText::new(name).size(14.0).color(t::SECONDARY));
                                    });
                                }
                                space(ui, 6.0);
                                text(
                                    ui,
                                    "Manette : maintenir Start pour basculer en mode souris ; Select + L1 + R1 + Start pour quitter.",
                                    13.0,
                                    t::SECONDARY,
                                );
                            }
                            5 => {
                                toggle(
                                    ui,
                                    "OSD statistiques (IPS, latence, débit, pertes)",
                                    &mut self.settings.osd,
                                );
                                toggle(
                                    ui,
                                    "Optimiser les réglages du jeu pour le streaming",
                                    &mut self.settings.game_optimization,
                                );
                                toggle(
                                    ui,
                                    "Fermer le programme en quittant le jeu",
                                    &mut self.settings.quit_after,
                                );
                                text(
                                    ui,
                                    "Sinon, le programme reste ouvert sur la place et vous le retrouvez à la prochaine connexion.",
                                    13.0,
                                    t::SECONDARY,
                                );
                            }
                            6 => {
                                text(ui, self.device.screen_label(), 15.0, t::PRIMARY);
                                space(ui, 10.0);
                                for (k, v) in [
                                    ("Processeur", self.device.cpu.as_str()),
                                    ("Mémoire", self.device.ram.as_str()),
                                    ("Carte graphique", self.device.gpu.as_str()),
                                    ("Décodeurs matériels", "Bientôt détecté"),
                                    ("Réseau", self.device.network.as_str()),
                                ] {
                                    ui.horizontal(|ui| {
                                        ui.add_sized([180.0, 27.0], egui::Label::new(k));
                                        text(ui, v, 14.0, t::SECONDARY);
                                    });
                                }
                                space(ui, 16.0);
                                ui.horizontal(|ui| {
                                    if action(ui, "Re-détecter", false) {
                                        self.device = Device::detect(ui.ctx());
                                    }
                                    if action(ui, "Tester ma connexion", false) {
                                        self.notice = Some(
                                            "Test simulé : aucune mesure réelle disponible".into(),
                                        );
                                    }
                                });
                            }
                            _ => {
                                text(
                                    ui,
                                    "PC lié : PC du salon (démonstration)",
                                    15.0,
                                    t::SECONDARY,
                                );
                                if action(ui, "Délier cet appareil", false) {
                                    self.navigate(Screen::Search);
                                }
                                space(ui, 18.0);
                                text(
                                    ui,
                                    format!(
                                        "Kinseat Client v{} · Licence GPLv3",
                                        env!("CARGO_PKG_VERSION")
                                    ),
                                    14.0,
                                    t::SECONDARY,
                                );
                            }
                        }
                        modified = self.settings != before;
                    });
                });
        });
        if modified {
            self.persist();
        }
    }
    fn capture(&mut self, ctx: &egui::Context) {
        let Some(path) = &self.screenshot else { return };
        for event in ctx.input(|i| i.events.clone()) {
            if let egui::Event::Screenshot { image, .. } = event {
                let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                if let Err(e) = image::save_buffer(
                    path,
                    &bytes,
                    image.size[0] as u32,
                    image.size[1] as u32,
                    image::ColorType::Rgba8,
                ) {
                    eprintln!("Capture impossible : {e}");
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
        }
        self.frames = self.frames.saturating_add(1);
        if self.frames >= 3 && !self.requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            self.requested = true;
        }
    }
}
impl eframe::App for Client {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.keyboard(ctx);
        if !self.demo {
            if self.frames < 2 {
                self.device = Device::detect(ctx);
            }
            self.real_update(ctx);
            self.update_panel(ctx);
            self.capture(ctx);
            return;
        }
        if self.frames < 2 {
            self.device = Device::detect(ctx);
        }
        if self.screen == Screen::Search
            && !self.discovered
            && self.started.elapsed() > Duration::from_secs(2)
        {
            self.discovered = true;
        }
        ctx.request_repaint_after(Duration::from_millis(100));
        if header(ctx, self.screen == Screen::Settings, Some("Démonstration")) {
            if self.screen == Screen::Settings {
                self.screen = self.return_to
            } else {
                self.return_to = self.screen;
                self.screen = Screen::Settings;
            }
        }
        footer(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t::CANVAS))
            .show(ctx, |ui| {
                let scroll = !matches!(self.screen, Screen::Stream | Screen::Settings);
                egui::ScrollArea::vertical()
                    .id_salt("page")
                    .auto_shrink([false, false])
                    .scroll_source(if scroll {
                        egui::scroll_area::ScrollSource::ALL
                    } else {
                        egui::scroll_area::ScrollSource::NONE
                    })
                    .show(ui, |ui| {
                        let width = (ui.available_width() - 80.0).clamp(640.0, 1040.0);
                        let margin = (ui.available_width() - width).max(0.0) / 2.0;
                        ui.add_space(30.0);
                        ui.horizontal(|ui| {
                            ui.add_space(margin);
                            ui.vertical(|ui| {
                                ui.set_width(width);
                                match self.screen {
                                    Screen::Search => self.search(ui),
                                    Screen::Pair => self.pair(ui),
                                    Screen::Members => self.members(ui),
                                    Screen::Pin => self.pin(ui),
                                    Screen::Home => self.home(ui),
                                    Screen::Stream => self.stream(ui),
                                    Screen::Settings => self.settings_ui(ui),
                                    Screen::Profile => self.profile_ui(ui),
                                    Screen::Waiting => self.waiting_ui(ui),
                                    Screen::Lock => self.lock_ui(ui),
                                }
                                if let Some(notice) = &self.notice {
                                    space(ui, 10.0);
                                    text(ui, notice, 13.0, t::ACCENT);
                                }
                            });
                        });
                        ui.add_space(24.0);
                    });
            });
        self.update_panel(ctx);
        self.capture(ctx);
    }
}
fn main() -> eframe::Result {
    let mut args = std::env::args().skip(1);
    let mut screen = Screen::Search;
    let mut demo = false;
    let mut demo_stream_menu = false;
    let mut category = 0;
    let mut screenshot = None;
    let mut session = Session::Available;
    let mut size = [1280.0, 720.0];
    let mut real_screen = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            // Capture d'un écran réel sans hôte (validation visuelle) : exige --screenshot.
            "--real-screen" => {
                real_screen = match args.next().as_deref() {
                    Some("pair" | "waiting") => Some(Screen::Waiting),
                    Some("profile") => Some(Screen::Profile),
                    Some("lock") => Some(Screen::Lock),
                    Some("home") => Some(Screen::Home),
                    Some("settings") => Some(Screen::Settings),
                    _ => {
                        eprintln!("--real-screen attend waiting|profile|lock|home|settings");
                        std::process::exit(2)
                    }
                };
            }
            "--demo-stream-menu" => {
                demo = true;
                demo_stream_menu = true;
                screen = Screen::Stream;
                size = [1280.0, 900.0];
            }
            "--demo-screen" => {
                demo = true;
                let n = args
                    .next()
                    .and_then(|v| v.parse::<u8>().ok())
                    .filter(|n| (1..=12).contains(n))
                    .unwrap_or_else(|| {
                        eprintln!("--demo-screen attend un nombre de 1 à 12");
                        std::process::exit(2)
                    });
                screen = Screen::from_number(n).unwrap();
                category = std::env::var("KINSEAT_DEMO_CATEGORY")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .filter(|c: &usize| *c < CATEGORIES.len())
                    .unwrap_or(match n {
                        8 => 4,
                        9 => 6,
                        _ => 0,
                    });
            }
            "--demo-state" => {
                session = match args.next().as_deref() {
                    Some("pause") => Session::Paused,
                    Some("occupe") => Session::Busy,
                    Some("libre") => Session::Available,
                    _ => {
                        eprintln!("--demo-state attend pause|occupe|libre");
                        std::process::exit(2)
                    }
                };
            }
            "--screenshot" => {
                screenshot = Some(PathBuf::from(args.next().unwrap_or_else(|| {
                    eprintln!("--screenshot attend un chemin");
                    std::process::exit(2)
                })))
            }
            "--window-size" => {
                let raw = args.next().unwrap_or_default();
                size = raw
                    .split_once('x')
                    .and_then(|(w, h)| Some([w.parse::<f32>().ok()?, h.parse::<f32>().ok()?]))
                    .filter(|s| s[0] >= 900.0 && s[1] >= 600.0)
                    .unwrap_or_else(|| {
                        eprintln!("--window-size attend WxH (min. 900x600)");
                        std::process::exit(2)
                    });
            }
            _ => {
                eprintln!("Argument inconnu : {arg}");
                std::process::exit(2)
            }
        }
    }
    if real_screen.is_some() && (demo || screenshot.is_none()) {
        eprintln!("--real-screen s'utilise avec --screenshot, sans --demo-screen");
        std::process::exit(2)
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/kinseat-64.png"))
                    .expect("icône Kinseat invalide"),
            )
            .with_inner_size(size)
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Kinseat Client",
        options,
        Box::new(move |cc| {
            let mut client = Client::new(
                screen,
                category,
                screenshot.clone(),
                session,
                demo,
                &cc.egui_ctx,
            );
            if demo_stream_menu {
                client.panel.open = true;
                client.stream_started = Some(Instant::now() - Duration::from_secs(15 * 60));
            }
            if let Some(screen) = real_screen {
                client.preview_real(screen);
            }
            Ok(Box::new(client))
        }),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn navigation() {
        assert_eq!(Screen::Pair.previous(), Screen::Search);
        assert_eq!(Screen::Stream.previous(), Screen::Home);
        assert_eq!(Screen::from_number(9), Some(Screen::Settings));
    }
    #[test]
    fn pair_and_pin() {
        let ctx = egui::Context::default();
        let mut c = Client::new(Screen::Pair, 0, None, Session::Available, true, &ctx);
        for d in "482731".chars() {
            c.digit(d);
        }
        c.submit();
        assert_eq!(c.screen, Screen::Members);
        c.submit();
        for d in "1234".chars() {
            c.digit(d);
        }
        c.submit();
        assert_eq!(c.screen, Screen::Home);
    }
}
