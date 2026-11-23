#![cfg_attr(windows, windows_subsystem = "windows")]
mod windows;
use egui::{Color32, RichText};
use std::path::PathBuf;
#[cfg(windows)]
include!(concat!(env!("OUT_DIR"), "/payload.rs"));

const CANVAS: Color32 = Color32::from_rgb(21, 20, 19);
const RAIL: Color32 = Color32::from_rgb(28, 26, 24);
const SURFACE: Color32 = Color32::from_rgb(35, 33, 30);
const HOVER: Color32 = Color32::from_rgb(46, 42, 38);
const AMBER: Color32 = Color32::from_rgb(232, 163, 61);
const TEXT: Color32 = Color32::from_rgb(237, 232, 225);
const SUB: Color32 = Color32::from_rgb(179, 171, 161);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Bienvenue,
    Options,
    Installation,
    Fin,
}
impl Screen {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "bienvenue" => Self::Bienvenue,
            "options" => Self::Options,
            "installation" => Self::Installation,
            "fin" => Self::Fin,
            _ => return None,
        })
    }
    fn index(self) -> usize {
        match self {
            Self::Bienvenue => 0,
            Self::Options => 1,
            Self::Installation => 2,
            Self::Fin => 3,
        }
    }
}
struct Setup {
    screen: Screen,
    demo: bool,
    uninstall: bool,
    desktop: bool,
    menu: bool,
    launch: bool,
    erase: bool,
    firewall: bool,
    notice: Option<String>,
    screenshot: Option<PathBuf>,
    requested: bool,
    frames: u8,
    pending: bool,
    #[cfg(windows)]
    install_rx: Option<std::sync::mpsc::Receiver<(Result<(), String>, Option<String>)>>,
    error: Option<String>,
}
impl Setup {
    fn new(
        screen: Screen,
        demo: bool,
        uninstall: bool,
        screenshot: Option<PathBuf>,
        ctx: &egui::Context,
    ) -> Self {
        ctx.set_theme(egui::ThemePreference::Dark);
        let mut visual = egui::Visuals::dark();
        visual.panel_fill = CANVAS;
        visual.window_fill = SURFACE;
        visual.widgets.inactive.bg_fill = HOVER;
        visual.widgets.inactive.weak_bg_fill = HOVER;
        visual.widgets.hovered.bg_fill = HOVER;
        visual.selection.bg_fill = AMBER;
        visual.window_shadow = egui::epaint::Shadow::NONE;
        ctx.set_visuals_of(egui::Theme::Dark, visual);
        #[cfg(windows)]
        {
            use egui::{FontData, FontDefinitions, FontFamily};
            if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
                let mut fonts = FontDefinitions::default();
                fonts.font_data.insert(
                    "segoe".into(),
                    std::sync::Arc::new(FontData::from_owned(bytes)),
                );
                fonts
                    .families
                    .get_mut(&FontFamily::Proportional)
                    .unwrap()
                    .insert(0, "segoe".into());
                ctx.set_fonts(fonts);
            }
        }
        Self {
            screen,
            demo,
            uninstall,
            desktop: false,
            menu: true,
            launch: true,
            erase: false,
            firewall: true,
            notice: None,
            screenshot,
            requested: false,
            frames: 0,
            pending: false,
            #[cfg(windows)]
            install_rx: None,
            error: None,
        }
    }
    fn label(ui: &mut egui::Ui, text: &str, size: f32, color: Color32) {
        ui.label(RichText::new(text).size(size).color(color));
    }
    fn button(ui: &mut egui::Ui, label: &str, primary: bool) -> bool {
        ui.add_sized(
            [170.0, 40.0],
            egui::Button::new(RichText::new(label).size(14.0).color(if primary {
                CANVAS
            } else {
                TEXT
            }))
            .fill(if primary { AMBER } else { HOVER })
            .corner_radius(6),
        )
        .clicked()
    }
    #[cfg(windows)]
    fn start_perform(&mut self, ctx: egui::Context) {
        let uninstall = self.uninstall;
        let erase = self.erase;
        let desktop = self.desktop;
        let menu = self.menu;
        let firewall = self.firewall;
        let (tx, rx) = std::sync::mpsc::channel();
        self.install_rx = Some(rx);
        std::thread::spawn(move || {
            let mut notice = None;
            let result = windows::paths().and_then(|p| {
                if uninstall {
                    windows::uninstall(&p, erase)
                } else {
                    windows::install(&p, desktop, menu)?;
                    if firewall {
                        if let Err(e) = windows::allow_firewall(&p) {
                            notice = Some(e);
                        }
                    }
                    Ok(())
                }
            });
            let _ = tx.send((result, notice));
            ctx.request_repaint();
        });
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
                    eprintln!("Capture : {e}");
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
impl eframe::App for Setup {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(windows)]
        {
            if self.pending && !self.demo {
                self.pending = false;
                self.start_perform(ctx.clone());
            }
            if let Some(rx) = &self.install_rx {
                if let Ok((result, notice)) = rx.try_recv() {
                    if let Err(e) = result {
                        self.error = Some(e);
                    }
                    self.notice = notice;
                    self.install_rx = None;
                    self.screen = Screen::Fin;
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_millis(80));
                }
            }
        }
        if self.screenshot.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
        egui::SidePanel::left("rail")
            .exact_width(210.0)
            .frame(egui::Frame::new().fill(RAIL).inner_margin(24))
            .show(ctx, |ui| {
                ui.add_space(18.0);
                Self::label(ui, "Kinseat", 22.0, AMBER);
                Self::label(ui, "CLIENT  /  INSTALLATION", 11.0, SUB);
                ui.add_space(54.0);
                for (n, title) in ["Bienvenue", "Options", "Installation", "Fin"]
                    .iter()
                    .enumerate()
                {
                    egui::Frame::new()
                        .fill(if n == self.screen.index() {
                            HOVER
                        } else {
                            RAIL
                        })
                        .corner_radius(6)
                        .inner_margin(12)
                        .show(ui, |ui| {
                            let color = if n == self.screen.index() { AMBER } else { SUB };
                            Self::label(ui, &format!("0{}   {title}", n + 1), 14.0, color);
                        });
                    ui.add_space(8.0);
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    Self::label(
                        ui,
                        concat!("VERSION ", env!("CARGO_PKG_VERSION"), " · GPLv3"),
                        11.0,
                        SUB,
                    )
                });
            });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(CANVAS).inner_margin(30)).show(ctx, |ui| {
            ui.add_space(20.0);
            Self::label(ui, "KINSEAT  /  CLIENT", 12.0, AMBER);
            ui.add_space(12.0);
            let title = match self.screen { Screen::Bienvenue => "Jouez depuis votre appareil", Screen::Options => "Vos préférences", Screen::Installation => "Installation", Screen::Fin => if self.error.is_some() { "Installation incomplète" } else { "Tout est prêt" } };
            Self::label(ui, title, 25.0, TEXT);
            ui.add_space(12.0);
            let subtitle = match self.screen { Screen::Bienvenue => "Kinseat Client rejoint votre PC familial : retrouvez votre place, avec l'image, le son, le clavier, la souris et la manette.", Screen::Options => "Installation dans votre session. Seule l'autorisation du pare-feu demande l'accord d'un administrateur.", Screen::Installation => "Préparation des fichiers et des raccourcis…", Screen::Fin => if self.error.is_some() { "Une étape n'a pas abouti. Vérifiez le message ci-dessous." } else { "Vous pouvez ouvrir Kinseat Client." } };
            ui.add(egui::Label::new(RichText::new(subtitle).size(14.0).color(SUB)).wrap());
            ui.add_space(28.0);
            egui::Frame::new().fill(SURFACE).corner_radius(10).inner_margin(22).show(ui, |ui| {
                ui.set_min_width(386.0);
                match self.screen {
                    Screen::Bienvenue => { Self::label(ui, "UNE PLACE POUR CHACUN", 12.0, AMBER); ui.add_space(12.0); Self::label(ui, "Retrouvez les joueurs du foyer, choisissez votre place et préparez votre expérience de jeu.", 15.0, TEXT); ui.add_space(14.0); Self::label(ui, "Aucun compte payant requis.", 13.0, SUB); }
                    Screen::Options => {
                        #[cfg(windows)] let folder = windows::paths().map(|p| p.program.display().to_string()).unwrap_or_else(|_| "%LOCALAPPDATA%\\Programs\\Kinseat Client".into());
                        #[cfg(not(windows))] let folder = "%LOCALAPPDATA%\\Programs\\Kinseat Client".to_owned();
                        Self::label(ui, "DOSSIER", 12.0, AMBER); ui.add_space(6.0); Self::label(ui, &folder, 13.0, TEXT); ui.add_space(17.0);
                        ui.checkbox(&mut self.desktop, "Raccourci sur le bureau");
                        ui.checkbox(&mut self.menu, "Raccourci dans le menu Démarrer");
                        if self.uninstall { ui.checkbox(&mut self.erase, "Supprimer aussi les réglages personnels"); }
                        else {
                            ui.checkbox(&mut self.firewall, "Autoriser Kinseat sur le réseau local (pare-feu Windows, accord administrateur demandé une fois)");
                            ui.checkbox(&mut self.launch, "Lancer à la fin");
                        }
                    }
                    Screen::Installation => { Self::label(ui, "FICHIERS  ·  RACCOURCIS  ·  ENREGISTREMENT", 13.0, AMBER); ui.add_space(16.0); ui.horizontal(|ui| { if !self.demo { ui.spinner(); } Self::label(ui, if self.demo { "Aucune modification en mode démonstration." } else { "Veuillez patienter…" }, 14.0, SUB); }); }
                    Screen::Fin => { Self::label(ui, if self.uninstall { "Désinstallation terminée" } else { concat!("Kinseat Client ", env!("CARGO_PKG_VERSION")) }, 17.0, TEXT); ui.add_space(12.0); if let Some(e) = &self.error { Self::label(ui, e, 13.0, AMBER); } else { Self::label(ui, if self.demo { "Capture de démonstration · aucun logiciel installé." } else if self.uninstall { "Kinseat Client a été retiré de cette session." } else { "Installation terminée. Ouvrez Kinseat Client, choisissez votre place et saisissez le code sur le PC hôte." }, 13.0, SUB); if let Some(n) = &self.notice { ui.add_space(8.0); Self::label(ui, n, 13.0, AMBER); } } }
                }
            });
            ui.add_space(24.0);
            ui.horizontal(|ui| {
                if self.screen == Screen::Options && Self::button(ui, "Retour", false) { self.screen = Screen::Bienvenue; }
                if self.screen == Screen::Bienvenue && Self::button(ui, "Continuer", true) { self.screen = Screen::Options; }
                if self.screen == Screen::Options && Self::button(ui, if self.uninstall { "Désinstaller" } else { "Installer" }, true) { self.screen = Screen::Installation; self.pending = true; }
                if self.screen == Screen::Installation && self.demo && Self::button(ui, "Terminer", true) { self.screen = Screen::Fin; }
                if self.screen == Screen::Fin && Self::button(ui, "Fermer", true) {
                    #[cfg(windows)] if !self.demo && !self.uninstall && self.launch && self.error.is_none() { if let Ok(p) = windows::paths() { let _ = std::process::Command::new(p.client).spawn(); } }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
        self.capture(ctx);
    }
}
fn main() -> eframe::Result {
    let mut args = std::env::args().skip(1);
    let (mut screen, mut demo, mut uninstall, mut screenshot) =
        (Screen::Bienvenue, false, false, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo-screen" => {
                screen = Screen::parse(&args.next().unwrap_or_default()).unwrap_or_else(|| {
                    eprintln!("Écran inconnu");
                    std::process::exit(2)
                });
                demo = true;
            }
            "--screenshot" => {
                screenshot = Some(PathBuf::from(args.next().unwrap_or_else(|| {
                    eprintln!("Chemin de capture absent");
                    std::process::exit(2)
                })))
            }
            "--uninstall" => uninstall = true,
            "--firewall" | "--erase-settings" => {}
            // Installation/désinstallation sans fenêtre (déploiement scripté) ; code de sortie 0 si réussi.
            "--silent" => {
                #[cfg(windows)]
                {
                    let result = windows::paths().and_then(|p| {
                        if uninstall {
                            windows::uninstall(
                                &p,
                                std::env::args().any(|a| a == "--erase-settings"),
                            )
                        } else {
                            windows::install(&p, false, true)?;
                            if std::env::args().any(|a| a == "--firewall") {
                                windows::allow_firewall(&p)?;
                            }
                            Ok(())
                        }
                    });
                    match result {
                        Ok(()) => std::process::exit(0),
                        Err(e) => {
                            eprintln!("{e}");
                            std::process::exit(1)
                        }
                    }
                }
                #[cfg(not(windows))]
                {
                    eprintln!("Installation Windows uniquement");
                    std::process::exit(2)
                }
            }
            _ => {
                eprintln!("Argument inconnu : {arg}");
                std::process::exit(2);
            }
        }
    }
    #[cfg(not(windows))]
    if !demo {
        eprintln!("Ce setup fonctionne uniquement sous Windows. Utilisez --demo-screen pour les captures.");
        std::process::exit(2);
    }
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([720.0, 460.0])
        .with_min_inner_size([720.0, 460.0])
        .with_max_inner_size([720.0, 460.0])
        .with_icon(
            eframe::icon_data::from_png_bytes(include_bytes!(
                "../../kinseat-client/assets/kinseat-64.png"
            ))
            .expect("Icône invalide"),
        );
    eframe::run_native(
        "Kinseat Client — Installation",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(move |cc| {
            Ok(Box::new(Setup::new(
                screen,
                demo,
                uninstall,
                screenshot.clone(),
                &cc.egui_ctx,
            )))
        }),
    )
}

#[cfg(test)]
mod build_guard {
    /// Chaque application egui doit activer elle-même les moteurs wgpu : la release compile
    /// chaque exe séparément, sans unification des fonctionnalités avec les autres crates.
    #[test]
    fn every_egui_app_enables_wgpu_backends() {
        for manifest in [
            include_str!("../Cargo.toml"),
            include_str!("../../kinseat-client/Cargo.toml"),
            include_str!("../../kinseat-setup/Cargo.toml"),
            include_str!("../../kinseat/Cargo.toml"),
        ] {
            let line = manifest
                .lines()
                .find(|l| l.starts_with("wgpu "))
                .expect("dépendance wgpu explicite");
            assert!(line.contains("\"dx12\""), "{line}");
        }
    }
}
