#![cfg_attr(windows, windows_subsystem = "windows")]
mod backend;
#[allow(dead_code)]
mod ops;
mod plan;
use egui::{Color32, RichText};
use kinseat_preflight::{evaluate, Check, LocalProbe, Probe, Statut};
use plan::{plan, set_multisession, Options};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};
const CANVAS: Color32 = Color32::from_rgb(21, 20, 19);
const RAIL: Color32 = Color32::from_rgb(28, 26, 24);
const SURFACE: Color32 = Color32::from_rgb(35, 33, 30);
const HOVER: Color32 = Color32::from_rgb(46, 42, 38);
const AMBER: Color32 = Color32::from_rgb(232, 163, 61);
const TEXT: Color32 = Color32::from_rgb(237, 232, 225);
const SUB: Color32 = Color32::from_rgb(179, 171, 161);
const GREEN: Color32 = Color32::from_rgb(110, 190, 120);
const RED: Color32 = Color32::from_rgb(232, 110, 100);
const VERSION: &str = concat!("VERSION ", env!("CARGO_PKG_VERSION"), " · GPLv3");
/// Avertissement repris dans Paramètres de l'application Hôte.
const MULTISESSION_WARNING: &str = "Nécessaire pour jouer à plusieurs en même temps sur ce PC : sans cette option, Windows n'ouvre qu'une session à la fois. RDP Wrapper est un composant tiers non officiel qui modifie le Bureau à distance de Windows (téléchargé depuis ses sources officielles pendant l'installation). Microsoft Defender peut le signaler : une exclusion limitée à son dossier est ajoutée. Une mise à jour de Windows peut le désactiver temporairement. Vous pourrez l'activer ou le retirer plus tard dans Paramètres.";
#[derive(serde::Deserialize)]
struct LogonTask {
    restricted: bool,
}

#[cfg(windows)]
fn installed_logon_tasks() -> std::io::Result<Vec<LogonTask>> {
    use std::io::{Read, Write};
    // Même protocole JSONL que kinseat-ipc ; le setup ne dépend pas de l'app installée.
    let mut pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(r"\\.\pipe\Kinseat.Admin.v1")?;
    pipe.write_all(b"{\"id\":1,\"method\":\"host.logonTasks.list\",\"params\":{}}\n")?;
    let mut line = Vec::new();
    let mut reader = (&mut pipe).take(4 * 1024 * 1024 + 1);
    loop {
        let mut byte = [0];
        if reader.read(&mut byte)? == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        }
        line.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    if line.len() > 4 * 1024 * 1024 {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
    }
    let response: serde_json::Value = serde_json::from_slice(&line)?;
    if response["id"] != 1 || !response["error"].is_null() {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
    }
    serde_json::from_value(response["result"].clone()).map_err(Into::into)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn logon_task_notice(tasks: &[LogonTask]) -> Option<String> {
    let count = tasks.iter().filter(|task| !task.restricted).count();
    match count {
        0 => None,
        1 => Some("1 programme de votre session s'ouvrira aussi à chaque démarrage de place. Kinseat vous proposera de le limiter à votre session.".into()),
        n => Some(format!("{n} programmes de votre session s'ouvriront aussi à chaque démarrage de place. Kinseat vous proposera de les limiter à votre session.")),
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn launch_arguments(open: bool, has_tasks: bool) -> &'static [&'static str] {
    if open && has_tasks {
        &["--logon-tasks"]
    } else {
        &[]
    }
}

#[cfg(windows)]
fn installed_app() -> PathBuf {
    PathBuf::from(r"C:\Program Files\Kinseat\kinseat.exe")
}

fn multisession_error_from_state(value: &serde_json::Value) -> Option<&str> {
    value
        .get("multisessionError")?
        .as_str()
        .filter(|s| !s.trim().is_empty())
}

fn installed_multisession_error() -> Option<String> {
    let path = PathBuf::from(std::env::var_os("ProgramData")?)
        .join("Kinseat")
        .join("install-state.json");
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    multisession_error_from_state(&value).map(str::to_owned)
}

const STEPS: [&str; 6] = [
    "Bienvenue",
    "Vérifications",
    "Composants",
    "Récapitulatif",
    "Installation",
    "Fin",
];
struct Setup {
    step: usize,
    pending: bool,
    checks: Option<Vec<Check>>,
    options: Options,
    consent: bool,
    error: Option<String>,
    tx: Sender<Vec<Check>>,
    rx: Receiver<Vec<Check>>,
    done_tx: Sender<Result<(), String>>,
    done_rx: Receiver<Result<(), String>>,
    tasks_tx: Sender<Option<String>>,
    tasks_rx: Receiver<Option<String>>,
    tasks_notice: Option<String>,
    open_app: bool,
    progress_file: Option<PathBuf>,
    log_file: Option<PathBuf>,
    uninstall: bool,
    demo: bool,
    screenshot: Option<PathBuf>,
    frames: u8,
    requested: bool,
}
impl Setup {
    fn new(
        cc: &eframe::CreationContext<'_>,
        step: usize,
        demo: bool,
        uninstall: bool,
        screenshot: Option<PathBuf>,
    ) -> Self {
        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = CANVAS;
        visuals.window_fill = SURFACE;
        visuals.widgets.inactive.bg_fill = HOVER;
        visuals.widgets.inactive.weak_bg_fill = HOVER;
        visuals.selection.bg_fill = AMBER;
        cc.egui_ctx.set_visuals_of(egui::Theme::Dark, visuals);
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
                cc.egui_ctx.set_fonts(fonts);
            }
        }
        let (tx, rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let (tasks_tx, tasks_rx) = mpsc::channel();
        Self {
            step,
            pending: false,
            checks: None,
            options: Options::default(),
            consent: false,
            error: None,
            tx,
            rx,
            done_tx,
            done_rx,
            tasks_tx,
            tasks_rx,
            tasks_notice: None,
            open_app: true,
            progress_file: None,
            log_file: None,
            uninstall,
            demo,
            screenshot,
            frames: 0,
            requested: false,
        }
    }
    fn label(ui: &mut egui::Ui, text: impl Into<String>, size: f32, color: Color32) {
        ui.label(RichText::new(text.into()).size(size).color(color));
    }
    fn button(ui: &mut egui::Ui, text: &str, primary: bool) -> bool {
        ui.add_sized(
            [145., 36.],
            egui::Button::new(RichText::new(text).color(if primary { CANVAS } else { TEXT }))
                .fill(if primary { AMBER } else { HOVER })
                .corner_radius(6),
        )
        .clicked()
    }
    fn refresh(&mut self, ctx: &egui::Context) {
        if self.pending {
            return;
        }
        self.pending = true;
        self.checks = None;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(evaluate(&LocalProbe.collect()));
            ctx.request_repaint();
        });
    }
    fn start(&mut self, ctx: &egui::Context) -> Result<(), String> {
        if self.demo {
            return Ok(());
        }
        let p = plan(self.options.clone())?;
        if !self.uninstall {
            let root = backend::payload_root()?;
            let manifest = backend::verify(&root)?;
            let warnings = backend::verify_optionals(&manifest, &root, &p);
            // Le processus élevé extrait et revérifie sa propre copie.
            backend::cleanup_payload(&root);
            if !warnings.is_empty() {
                self.error = Some(warnings.join(" ; "));
            }
        }
        let dir = std::env::temp_dir().join(format!("kinseat-setup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let file = dir.join("plan.json");
        let progress = dir.join("progress.jsonl");
        let log = dir.join("install.log");
        std::fs::write(
            &file,
            serde_json::to_vec_pretty(&p).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(&progress, b"").map_err(|e| e.to_string())?;
        self.progress_file = Some(progress.clone());
        self.log_file = Some(log.clone());
        let tx = self.done_tx.clone();
        let ctx = ctx.clone();
        let uninstall = self.uninstall;
        std::thread::spawn(move || {
            let result = backend::elevate(&file, &progress, &log, uninstall);
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        Ok(())
    }
    fn item(ui: &mut egui::Ui, color: Color32, title: &str, detail: &str, advice: Option<&str>) {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(10., 10.), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 5., color);
            ui.add_space(4.);
            ui.label(RichText::new(title).size(14.).color(TEXT));
            if !detail.is_empty() {
                ui.label(RichText::new(format!("·  {detail}")).size(13.).color(SUB));
            }
        });
        if let Some(advice) = advice {
            ui.horizontal(|ui| {
                ui.add_space(22.);
                ui.add(egui::Label::new(RichText::new(advice).size(12.).color(SUB)).wrap());
            });
        }
        ui.add_space(9.);
    }
    fn body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        match self.step {
            0 => {
                Self::label(ui, "UNE PLACE POUR CHACUN", 12., AMBER);
                ui.add_space(10.);
                ui.add(egui::Label::new(RichText::new("Avec Kinseat, chaque personne de la maison joue sur sa propre place, depuis un autre appareil (PC, tablette, télé), avec ses jeux et ses sauvegardes, pendant que vous gardez ce PC.").size(14.).color(TEXT)).wrap());
                ui.add_space(10.);
                ui.add(egui::Label::new(RichText::new("L'assistant vérifie d'abord ce PC, puis installe tout en une fois. Windows demandera une seule autorisation administrateur. Rien n'est modifié avant votre confirmation.").size(13.).color(SUB)).wrap());
            }
            1 => {
                if self.pending {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        Self::label(ui, "Vérification en cours…", 13., SUB);
                    });
                }
                if let Some(checks) = &self.checks {
                    for c in checks.iter().filter(|c| c.id != "admin") {
                        let color = match c.statut {
                            Statut::Ok => GREEN,
                            Statut::Attention => AMBER,
                            Statut::Bloquant => RED,
                        };
                        let advice = (c.statut != Statut::Ok).then_some(c.conseil);
                        Self::item(ui, color, c.titre, &c.valeur_trouvee, advice);
                    }
                }
                if !self.pending && ui.button("Relancer les vérifications").clicked() {
                    self.refresh(ctx)
                }
            }
            2 => {
                ui.add(egui::Label::new(RichText::new("Tout est nécessaire au bon fonctionnement de Kinseat : rien à choisir.").size(14.).color(TEXT)).wrap());
                ui.add_space(14.);
                for (title, detail) in [
                    (
                        "Service Kinseat et application Hôte",
                        "gère les places et les sessions",
                    ),
                    ("ApolloVibe", "diffuse le jeu vers les appareils"),
                    ("HidHide", "chaque manette reste sur sa place"),
                    ("Raccourcis", "Bureau et menu Démarrer"),
                    ("Pare-feu", "l'accès Bureau à distance reste limité à ce PC"),
                ] {
                    Self::item(ui, GREEN, title, detail, None);
                }
                ui.add_space(6.);
                Self::label(ui, "Dossier : C:\\Program Files\\Kinseat", 12., SUB);
                ui.add_space(16.);
                Self::label(ui, "OPTION EXPÉRIMENTALE", 12., AMBER);
                ui.add_space(6.);
                let mut multi = self.options.rdpwrap;
                if ui
                    .checkbox(
                        &mut multi,
                        RichText::new(
                            "Permettre plusieurs sessions Windows en même temps (RDP Wrapper)",
                        )
                        .size(14.)
                        .color(TEXT),
                    )
                    .changed()
                {
                    set_multisession(&mut self.options, multi);
                }
                ui.horizontal(|ui| {
                    ui.add_space(24.);
                    ui.add(
                        egui::Label::new(RichText::new(MULTISESSION_WARNING).size(12.).color(SUB))
                            .wrap(),
                    );
                });
            }
            3 => {
                if self.uninstall {
                    ui.add(egui::Label::new(RichText::new("Kinseat sera retiré de ce PC : service, application, raccourcis, règles de pare-feu et HidHide. Les réglages Bureau à distance d'origine sont restaurés. Vos données (places, membres) sont conservées.").size(13.).color(TEXT)).wrap());
                } else {
                    match plan(self.options.clone()) {
                        Ok(p) => {
                            for step in p.steps {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!("•  {step}")).size(13.).color(TEXT),
                                    )
                                    .wrap(),
                                );
                                ui.add_space(3.);
                            }
                        }
                        Err(e) => Self::label(ui, e, 13., RED),
                    }
                }
                ui.add_space(12.);
                ui.checkbox(
                    &mut self.consent,
                    RichText::new(
                        "Je confirme et j'accepte une demande d'autorisation administrateur",
                    )
                    .size(13.)
                    .color(TEXT),
                );
            }
            4 => {
                if let Some(path) = &self.progress_file {
                    let rows = backend::progress_lines(path);
                    if let Some(last) = rows.last() {
                        ui.add(
                            egui::ProgressBar::new((last.pct / 100.).clamp(0., 1.))
                                .fill(AMBER)
                                .text(last.message.clone()),
                        );
                        ui.add_space(8.);
                        for row in rows.iter().rev().take(12) {
                            let color = match row.status.as_str() {
                                "error" => RED,
                                "warning" => AMBER,
                                _ => SUB,
                            };
                            Self::label(ui, format!("{} · {}", row.step, row.message), 12., color);
                        }
                    } else {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            Self::label(
                                ui,
                                "En attente de l'autorisation administrateur…",
                                13.,
                                SUB,
                            );
                        });
                    }
                    ctx.request_repaint_after(Duration::from_millis(250));
                } else {
                    Self::label(ui, "Démo : aucune installation exécutée.", 13., SUB);
                }
            }
            _ => {
                if let Some(e) = &self.error {
                    ui.add(egui::Label::new(RichText::new(e).size(13.).color(RED)).wrap());
                } else {
                    Self::label(
                        ui,
                        if self.demo {
                            "Démo : aucune installation exécutée."
                        } else if self.uninstall {
                            "Kinseat a été retiré de ce PC."
                        } else {
                            "Kinseat est installé. Ouvrez « Kinseat Hôte » depuis le Bureau pour créer les places."
                        },
                        14.,
                        TEXT,
                    );
                }
                if !self.demo && !self.uninstall {
                    if let Some(message) = installed_multisession_error() {
                        ui.add_space(12.);
                        egui::Frame::new()
                            .fill(SURFACE)
                            .inner_margin(12.)
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!(
                                            "Plusieurs sessions Windows : {message}"
                                        ))
                                        .size(13.)
                                        .color(AMBER),
                                    )
                                    .wrap(),
                                );
                            });
                    }
                }
                if !self.demo && !self.uninstall && self.error.is_none() {
                    if let Some(message) = &self.tasks_notice {
                        ui.add_space(12.);
                        ui.add(
                            egui::Label::new(RichText::new(message).size(13.).color(AMBER)).wrap(),
                        );
                    }
                    ui.add_space(12.);
                    ui.checkbox(&mut self.open_app, "Ouvrir Kinseat");
                }
                if let Some(path) = &self.progress_file {
                    for row in backend::progress_lines(path)
                        .iter()
                        .filter(|r| r.status == "warning")
                    {
                        ui.add_space(6.);
                        ui.add(
                            egui::Label::new(RichText::new(&row.message).size(13.).color(AMBER))
                                .wrap(),
                        );
                    }
                }
                if let Some(log) = &self.log_file {
                    ui.add_space(8.);
                    if ui.button("Ouvrir le journal").clicked() {
                        #[cfg(windows)]
                        {
                            let _ = std::process::Command::new("explorer.exe")
                                .arg(format!("/select,{}", log.display()))
                                .spawn();
                        }
                    }
                    Self::label(ui, log.display().to_string(), 11., SUB);
                }
            }
        }
    }
    fn nav(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            if self.step > 0
                && self.step < 4
                && !self.uninstall
                && Self::button(ui, "Précédent", false)
            {
                self.step -= 1;
            }
            let ready = match self.step {
                0 | 2 => true,
                1 => {
                    self.demo
                        || (!self.pending
                            && self.checks.as_ref().is_some_and(|c| {
                                !c.iter()
                                    .any(|x| x.statut == Statut::Bloquant && x.id != "admin")
                            }))
                }
                3 => self.consent && (self.uninstall || plan(self.options.clone()).is_ok()),
                _ => false,
            };
            let label = match self.step {
                3 if self.uninstall => "Désinstaller",
                3 => "Installer",
                _ => "Continuer",
            };
            if ready && Self::button(ui, label, true) {
                self.step += 1;
                if self.step == 1 && !self.demo {
                    self.refresh(ctx)
                }
                if self.step == 4 {
                    if let Err(e) = self.start(ctx) {
                        self.error = Some(e);
                        self.step = 5;
                    }
                }
            }
            if self.step == 4 && self.demo && Self::button(ui, "Terminer", true) {
                self.step = 5
            }
            if self.step == 5 && Self::button(ui, "Fermer", true) {
                #[cfg(windows)]
                if !self.demo && !self.uninstall && self.error.is_none() && self.open_app {
                    // L'ouverture est facultative : un échec ne change pas le résultat de l'installation.
                    let _ = std::process::Command::new(installed_app())
                        .args(launch_arguments(self.open_app, self.tasks_notice.is_some()))
                        .spawn();
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
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
                    eprintln!("Capture : {e}")
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
        }
        self.frames = self.frames.saturating_add(1);
        ctx.request_repaint();
        // Démo de l'étape Vérifications : la sonde (lecture seule) tourne vraiment, capture après résultat.
        if self.step == 1 && self.checks.is_none() && !self.pending && self.frames == 1 {
            self.refresh(ctx);
        }
        if self.frames >= 3 && !self.pending && !self.requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            self.requested = true
        }
    }
}
impl eframe::App for Setup {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        while let Ok(checks) = self.rx.try_recv() {
            self.pending = false;
            self.checks = Some(checks)
        }
        if let Ok(result) = self.done_rx.try_recv() {
            if result.is_ok() && !self.uninstall && !self.demo {
                let tx = self.tasks_tx.clone();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    #[cfg(windows)]
                    let notice = installed_logon_tasks()
                        .ok()
                        .and_then(|tasks| logon_task_notice(&tasks));
                    #[cfg(not(windows))]
                    let notice = None;
                    let _ = tx.send(notice);
                    ctx.request_repaint();
                });
            }
            if let Err(e) = result {
                self.error = Some(e)
            }
            self.step = 5;
        }
        if let Ok(notice) = self.tasks_rx.try_recv() {
            self.tasks_notice = notice;
        }
        egui::SidePanel::left("rail")
            .exact_width(210.)
            .frame(egui::Frame::new().fill(RAIL).inner_margin(24))
            .show(ctx, |ui| {
                ui.add_space(14.);
                Self::label(ui, "Kinseat", 22., AMBER);
                Self::label(ui, "HÔTE  /  INSTALLATION", 11., SUB);
                ui.add_space(38.);
                for (n, title) in STEPS.iter().enumerate() {
                    egui::Frame::new()
                        .fill(if n == self.step { HOVER } else { RAIL })
                        .corner_radius(6)
                        .inner_margin(9)
                        .show(ui, |ui| {
                            Self::label(
                                ui,
                                format!("0{}   {title}", n + 1),
                                13.,
                                if n == self.step { AMBER } else { SUB },
                            )
                        });
                    ui.add_space(5.);
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    Self::label(ui, VERSION, 11., SUB)
                });
            });
        egui::TopBottomPanel::bottom("nav")
            .frame(egui::Frame::new().fill(CANVAS).inner_margin(egui::Margin {
                left: 25,
                right: 25,
                top: 12,
                bottom: 20,
            }))
            .show_separator_line(false)
            .show(ctx, |ui| self.nav(ui, ctx));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(CANVAS).inner_margin(25))
            .show(ctx, |ui| {
                Self::label(ui, "KINSEAT  /  HÔTE", 12., AMBER);
                ui.add_space(8.);
                Self::label(ui, STEPS[self.step], 25., TEXT);
                ui.add_space(8.);
                let subtitle = match self.step {
                    0 => "Préparer ce PC pour jouer à plusieurs.",
                    1 => "Ce que Kinseat a trouvé sur ce PC. Rien n'est encore modifié.",
                    2 => "Ce qui va être installé.",
                    3 => "Dernière relecture avant l'installation.",
                    4 => "Installation en cours, ne fermez pas cette fenêtre.",
                    _ => {
                        if self.error.is_some() {
                            "L'installation n'a pas abouti."
                        } else {
                            "C'est prêt."
                        }
                    }
                };
                ui.add(egui::Label::new(RichText::new(subtitle).color(SUB)).wrap());
                ui.add_space(17.);
                egui::Frame::new()
                    .fill(SURFACE)
                    .corner_radius(10)
                    .inner_margin(18)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.body(ui, ctx));
                    });
            });
        self.capture(ctx);
    }
}
fn main() -> eframe::Result {
    let args: Vec<_> = std::env::args_os().collect();
    #[cfg(windows)]
    if args
        .iter()
        .any(|a| a == "--apply" || a == "--print-plan" || a == "--preflight-json")
    {
        // Application fenêtrée : se raccrocher au terminal appelant pour les modes ligne de commande.
        unsafe {
            windows_sys::Win32::System::Console::AttachConsole(
                windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
            )
        };
    }
    if let Some(pos) = args.iter().position(|a| a == "--apply") {
        let value = |flag: &str| -> Option<PathBuf> {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1))
                .map(PathBuf::from)
        };
        let result = match (
            args.get(pos + 1).map(PathBuf::from),
            value("--progress"),
            value("--log"),
        ) {
            (Some(plan), Some(progress), Some(log)) => {
                let result = backend::run_apply(
                    &plan,
                    args.iter().any(|a| a == "--uninstall"),
                    &progress,
                    &log,
                );
                if let Err(e) = &result {
                    let row =
                        serde_json::json!({"step":"complete","status":"error","message":e,"pct":0});
                    let mut bytes = serde_json::to_vec(&row).unwrap_or_default();
                    bytes.push(b'\n');
                    let _ = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&progress)
                        .and_then(|mut f| std::io::Write::write_all(&mut f, &bytes));
                }
                result
            }
            _ => Err("Arguments --apply incomplets".into()),
        };
        if let Err(e) = result {
            eprintln!("{e}");
            std::process::exit(1)
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--print-plan") {
        // Plan par défaut (installation sans assistant, ex. déploiement scripté) :
        // setup.exe --print-plan > plan.json puis setup.exe --apply plan.json --progress p --log l
        let plan = plan(Options::default()).expect("plan par défaut valide");
        println!(
            "{}",
            serde_json::to_string_pretty(&plan).expect("sérialisation")
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--preflight-json") {
        println!(
            "{}",
            serde_json::to_string(&evaluate(&LocalProbe.collect())).expect("sérialisation")
        );
        return Ok(());
    }
    let mut step = 0;
    let mut demo = false;
    let mut uninstall = false;
    let mut screenshot = None;
    let mut demo_multisession = false;
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--demo-screen" => {
                demo = true;
                let wanted = iter.next().unwrap_or_default().to_lowercase();
                step = STEPS
                    .iter()
                    .position(|s| s.to_lowercase() == wanted)
                    .unwrap_or(0)
            }
            "--screenshot" => screenshot = iter.next().map(PathBuf::from),
            "--demo-multisession" => demo_multisession = true,
            "--uninstall" => {
                uninstall = true;
                step = 3
            }
            _ => {
                eprintln!("Argument inconnu : {arg}");
                std::process::exit(2)
            }
        }
    }
    #[cfg(not(windows))]
    if !demo {
        eprintln!("Installation Windows uniquement");
        std::process::exit(2)
    }
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([900., 640.])
        .with_min_inner_size([820., 600.])
        .with_icon(
            eframe::icon_data::from_png_bytes(include_bytes!(
                "../../kinseat/assets/kinseat-256.png"
            ))
            .unwrap_or_default(),
        );
    eframe::run_native(
        "Kinseat Hôte — Installation",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(move |cc| {
            let mut setup = Setup::new(cc, step, demo, uninstall, screenshot.clone());
            if demo_multisession {
                set_multisession(&mut setup.options, true);
            }
            Ok(Box::new(setup))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multisession_error_optional() {
        assert_eq!(multisession_error_from_state(&serde_json::json!({})), None);
        let value = serde_json::json!({"multisessionError":"Windows non pris en charge"});
        assert_eq!(
            multisession_error_from_state(&value),
            Some("Windows non pris en charge")
        );
    }

    #[test]
    fn install_notice_and_launch_arguments() {
        let mut tasks: Vec<LogonTask> = serde_json::from_value(serde_json::json!([
            {"path":"\\", "name":"A", "execute":"A.exe", "runAs":"hôte", "restricted":false},
            {"path":"\\", "name":"B", "execute":"B.exe", "runAs":"hôte", "restricted":true},
            {"path":"\\", "name":"C", "execute":"C.exe", "runAs":"hôte", "restricted":false}
        ]))
        .unwrap();
        assert_eq!(logon_task_notice(&tasks[..1]).as_deref(), Some("1 programme de votre session s'ouvrira aussi à chaque démarrage de place. Kinseat vous proposera de le limiter à votre session."));
        assert!(logon_task_notice(&tasks[1..2]).is_none());
        assert!(logon_task_notice(&[]).is_none());
        assert_eq!(logon_task_notice(&tasks).as_deref(), Some("2 programmes de votre session s'ouvriront aussi à chaque démarrage de place. Kinseat vous proposera de les limiter à votre session."));
        tasks[2].restricted = true;
        assert_eq!(
            launch_arguments(true, logon_task_notice(&tasks).is_some()),
            &["--logon-tasks"]
        );
        assert!(launch_arguments(true, false).is_empty());
        assert!(launch_arguments(false, true).is_empty());
    }
}
