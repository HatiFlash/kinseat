//! Paramètres → « Plusieurs sessions Windows » (RDP Wrapper, expérimental).
//! L'activation passe par `MultiSession-Kinseat.ps1` (installé à côté de kinseat.exe), lancé en
//! administrateur sans fenêtre ; la progression est lue dans %ProgramData%\Kinseat\multisession.jsonl.
use crate::theme;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Même avertissement que dans le setup Hôte.
pub const WARNING: &str = "Nécessaire pour jouer à plusieurs en même temps sur ce PC : sans cette option, Windows n'ouvre qu'une session à la fois. RDP Wrapper est un composant tiers non officiel qui modifie le Bureau à distance de Windows (téléchargé depuis ses sources officielles). Microsoft Defender peut le signaler : une exclusion limitée à son dossier est ajoutée. Une mise à jour de Windows peut le désactiver temporairement : utilisez alors « Réparer ».";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Windows d'origine : une seule session à la fois.
    Inactive,
    /// RDP Wrapper installé par Kinseat.
    Active,
    /// RDP Wrapper/TermWrap installé hors Kinseat : on n'y touche pas.
    External,
}

/// `ServiceDll` de TermService : le termsrv.dll d'origine ou un composant multi-session.
pub fn is_stock_service_dll(dll: Option<&str>) -> bool {
    match dll {
        None => true,
        Some(d) => d
            .trim()
            .to_ascii_lowercase()
            .replace('/', "\\")
            .ends_with("\\system32\\termsrv.dll"),
    }
}

pub fn classify(service_dll: Option<&str>, owned_marker: bool) -> State {
    if is_stock_service_dll(service_dll) {
        State::Inactive
    } else if owned_marker {
        State::Active
    } else {
        State::External
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub step: String,
    pub status: String,
    pub message: String,
}

/// Lignes JSON complètes du fichier de progression (BOM et ligne incomplète ignorés).
pub fn parse_progress(text: &str) -> Vec<Row> {
    text.trim_start_matches('\u{feff}')
        .lines()
        .filter_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
            Some(Row {
                step: v.get("step")?.as_str()?.to_owned(),
                status: v.get("status")?.as_str()?.to_owned(),
                message: v.get("message")?.as_str().unwrap_or_default().to_owned(),
            })
        })
        .collect()
}

fn program_files() -> PathBuf {
    PathBuf::from(std::env::var_os("ProgramFiles").unwrap_or_else(|| r"C:\Program Files".into()))
}
fn progress_file() -> PathBuf {
    PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into()))
        .join("Kinseat")
        .join("multisession.jsonl")
}
fn marker() -> PathBuf {
    program_files()
        .join("RDP Wrapper")
        .join("kinseat-owned.json")
}
fn script() -> Option<PathBuf> {
    let path = std::env::current_exe()
        .ok()?
        .parent()?
        .join("MultiSession-Kinseat.ps1");
    path.exists().then_some(path)
}

#[cfg(windows)]
fn service_dll() -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    };
    let key: Vec<u16> = r"SYSTEM\CurrentControlSet\Services\TermService\Parameters"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let name: Vec<u16> = "ServiceDll".encode_utf16().chain(Some(0)).collect();
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if rc != 0 {
        return None;
    }
    let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}
#[cfg(not(windows))]
fn service_dll() -> Option<String> {
    None
}

pub fn detect() -> State {
    classify(service_dll().as_deref(), marker().exists())
}

#[cfg(windows)]
fn launch(script: &Path, enable: bool) -> std::io::Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let exe = wide(&format!(
        r"{}\System32\WindowsPowerShell\v1.0\powershell.exe",
        std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())
    ));
    let args = wide(&format!(
        "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File \"{}\" -Action {} -UpdateState",
        script.display(),
        if enable { "Enable" } else { "Disable" }
    ));
    let verb = wide("runas");
    // SW_HIDE (0) : aucune fenêtre ; ≤ 32 = échec, y compris UAC refusé.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            exe.as_ptr(),
            args.as_ptr(),
            std::ptr::null(),
            0,
        )
    };
    if result as isize <= 32 {
        return Err(std::io::Error::other(
            "Autorisation administrateur refusée ou annulée.",
        ));
    }
    Ok(())
}
#[cfg(not(windows))]
fn launch(_: &Path, _: bool) -> std::io::Result<()> {
    Err(std::io::Error::other("Windows uniquement"))
}

struct Running {
    enable: bool,
    since: SystemTime,
    started: Instant,
    rows: Vec<Row>,
}

pub struct Panel {
    state: Option<State>,
    checked: Option<Instant>,
    confirm: Option<bool>,
    running: Option<Running>,
    result: Option<(bool, String)>,
}

impl Panel {
    pub fn new(demo: bool) -> Self {
        Self {
            state: demo.then_some(State::Inactive),
            checked: None,
            confirm: None,
            running: None,
            result: None,
        }
    }

    fn poll(&mut self, demo: bool) {
        if demo {
            return;
        }
        if let Some(run) = &mut self.running {
            let path = progress_file();
            let fresh = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .is_ok_and(|t| t >= run.since);
            if fresh {
                run.rows = parse_progress(&std::fs::read_to_string(&path).unwrap_or_default());
            }
            if let Some(done) = run.rows.iter().rev().find(|r| r.step == "complete") {
                let ok = done.status == "ok";
                self.result = Some((
                    ok,
                    if ok {
                        done.message.clone()
                    } else {
                        format!("Échec : {}", done.message)
                    },
                ));
                self.running = None;
                self.checked = None;
            } else if run.started.elapsed() > Duration::from_secs(600) {
                self.result = Some((false, "Pas de réponse après 10 minutes : consultez %ProgramData%\\Kinseat\\multisession.log.".into()));
                self.running = None;
                self.checked = None;
            }
        }
        if self.running.is_none()
            && self
                .checked
                .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
        {
            self.state = Some(detect());
            self.checked = Some(Instant::now());
        }
    }

    fn start(&mut self, enable: bool, demo: bool) {
        self.result = None;
        if demo {
            self.result = Some((true, "Démonstration : aucune modification.".into()));
            return;
        }
        let Some(script) = script() else {
            self.result = Some((false, "MultiSession-Kinseat.ps1 introuvable : réinstallez Kinseat Hôte 0.3.6 ou plus récent.".into()));
            return;
        };
        let since = SystemTime::now();
        match launch(&script, enable) {
            Ok(()) => {
                self.running = Some(Running {
                    enable,
                    since,
                    started: Instant::now(),
                    rows: Vec::new(),
                })
            }
            Err(e) => self.result = Some((false, e.to_string())),
        }
    }

    /// `service_active` : avis du service Kinseat (RDP Wrapper chargé et fonctionnel).
    pub fn show(&mut self, ui: &mut egui::Ui, demo: bool, service_active: Option<bool>) {
        self.poll(demo);
        if self.running.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
        theme::frame().inner_margin(24).show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));
            ui.label(theme::title("Plusieurs sessions Windows", 19.0));
            ui.add_space(6.0);
            ui.label(theme::text("Option expérimentale (RDP Wrapper)", 13.0, theme::ACCENT));
            ui.add_space(12.0);
            let state = self.state;
            let broken = state == Some(State::Active) && service_active == Some(false);
            let (label, color) = match state {
                None => ("Vérification…", theme::SECONDARY),
                Some(State::Inactive) => ("Inactive : une seule session Windows à la fois", theme::WARN),
                Some(State::Active) if broken => ("Installée mais inopérante (mise à jour de Windows ?)", theme::WARN),
                Some(State::Active) => ("Active : plusieurs places peuvent jouer en même temps", theme::OK),
                Some(State::External) => ("Active (composant installé hors Kinseat)", theme::OK),
            };
            ui.label(theme::text(label, 14.0, color));
            ui.add_space(8.0);
            ui.label(theme::text(WARNING, 13.0, theme::SECONDARY));
            ui.add_space(14.0);
            if let Some(run) = &self.running {
                ui.horizontal(|ui| {
                    ui.spinner();
                    let current = run.rows.last().map(|r| r.message.as_str()).unwrap_or("En attente de l'autorisation administrateur…");
                    ui.label(theme::text(if run.enable { format!("Activation · {current}") } else { format!("Désactivation · {current}") }, 14.0, theme::SECONDARY));
                });
                return;
            }
            if let Some(enable) = self.confirm {
                ui.label(theme::text(
                    if enable {
                        "Confirmer ? Windows demandera une autorisation administrateur. Les places ouvertes seront déconnectées pendant le redémarrage du Bureau à distance."
                    } else {
                        "Retirer RDP Wrapper ? Les places ouvertes seront déconnectées et une seule session Windows sera de nouveau possible."
                    },
                    14.0,
                    theme::PRIMARY,
                ));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let ok = egui::Button::new(theme::title(if enable { "Confirmer l'activation" } else { "Confirmer le retrait" }, 14.0).color(theme::ON_ACCENT)).fill(theme::ACCENT).corner_radius(6).min_size(egui::vec2(200.0, 36.0));
                    if ui.add(ok).clicked() {
                        self.confirm = None;
                        self.start(enable, demo);
                    }
                    if ui.add(egui::Button::new("Annuler").corner_radius(6).min_size(egui::vec2(120.0, 36.0))).clicked() {
                        self.confirm = None;
                    }
                });
            } else {
                let action = match state {
                    Some(State::Inactive) => Some((true, "Activer")),
                    Some(State::Active) if broken => Some((true, "Réparer")),
                    _ => None,
                };
                ui.horizontal(|ui| {
                    if let Some((enable, text)) = action {
                        let b = egui::Button::new(theme::title(text, 14.0).color(theme::ON_ACCENT)).fill(theme::ACCENT).corner_radius(6).min_size(egui::vec2(160.0, 36.0));
                        if ui.add(b).clicked() {
                            self.confirm = Some(enable);
                        }
                    }
                    if state == Some(State::Active) && ui.add(egui::Button::new("Désactiver").corner_radius(6).min_size(egui::vec2(140.0, 36.0))).clicked() {
                        self.confirm = Some(false);
                    }
                });
                if state == Some(State::External) {
                    ui.label(theme::text("Kinseat ne modifie pas un composant qu'il n'a pas installé.", 13.0, theme::SECONDARY));
                }
            }
            if let Some((ok, message)) = &self.result {
                ui.add_space(10.0);
                ui.label(theme::text(message, 14.0, if *ok { theme::OK } else { theme::ERR }));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stock_dll_detection() {
        assert!(is_stock_service_dll(Some(
            "%SystemRoot%\\System32\\termsrv.dll"
        )));
        assert!(is_stock_service_dll(Some(
            "C:\\Windows\\system32\\TermSrv.dll"
        )));
        assert!(is_stock_service_dll(None));
        assert!(!is_stock_service_dll(Some(
            "C:\\Program Files\\RDP Wrapper\\rdpwrap.dll"
        )));
        assert!(!is_stock_service_dll(Some(
            "C:\\Program Files\\TermWrap\\termwrap.dll"
        )));
        assert_eq!(
            classify(Some("C:\\Program Files\\RDP Wrapper\\rdpwrap.dll"), true),
            State::Active
        );
        assert_eq!(
            classify(Some("C:\\Program Files\\RDP Wrapper\\rdpwrap.dll"), false),
            State::External
        );
        assert_eq!(
            classify(Some("%SystemRoot%\\System32\\termsrv.dll"), true),
            State::Inactive
        );
    }
    #[test]
    fn progress_parsing_skips_bom_and_partial() {
        let rows = parse_progress("\u{feff}{\"step\":\"check\",\"status\":\"running\",\"message\":\"Vérification\",\"pct\":5}\r\n{\"step\":\"complete\",\"status\":\"ok\",\"message\":\"fait\",\"pct\":100}\r\n{\"step\":");
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[1],
            Row {
                step: "complete".into(),
                status: "ok".into(),
                message: "fait".into()
            }
        );
    }
}
