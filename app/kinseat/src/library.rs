use crate::{session::Command, theme as t, views};
use kinseat_ipc::{Account, Library};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const ADD_KEY: &str = "\0add";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingAction {
    Add,
    Remove,
    Update,
}

impl PendingAction {
    fn label(self) -> &'static str {
        match self {
            Self::Add => "Ajout…",
            Self::Remove => "Retrait…",
            Self::Update => "Mise à jour…",
        }
    }
}

#[derive(Default)]
pub struct Pending {
    actions: HashMap<String, PendingAction>,
}

impl Pending {
    fn begin(&mut self, key: &str, action: PendingAction) -> bool {
        if self.actions.contains_key(key) {
            return false;
        }
        self.actions.insert(key.to_owned(), action);
        true
    }

    pub fn finish(&mut self, key: &str) {
        self.actions.remove(key);
    }

    fn get(&self, key: &str) -> Option<PendingAction> {
        self.actions.get(key).copied()
    }

    pub fn any(&self) -> bool {
        !self.actions.is_empty()
    }
}

fn progress(ui: &mut egui::Ui, pending: PendingAction) {
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new());
        ui.label(pending.label());
    });
}

#[derive(Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOptions {
    #[serde(default = "default_auto")]
    pub save_isolation: String,
    #[serde(default)]
    pub start_from_host_save: bool,
    #[serde(default)]
    pub anti_cheat: bool,
    #[serde(default)]
    pub save_location: String,
    #[serde(default)]
    pub store: Option<String>,
}

fn default_auto() -> String {
    "auto".into()
}

pub fn parse_library(
    value: serde_json::Value,
) -> serde_json::Result<(Library, HashMap<String, SaveOptions>)> {
    let options = value
        .get("games")
        .and_then(|v| v.as_array())
        .map(|games| {
            games
                .iter()
                .filter_map(|game| {
                    let id = game.get("id")?.as_str()?.to_owned();
                    let options = serde_json::from_value(game.clone()).ok()?;
                    Some((id, options))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok((serde_json::from_value(value)?, options))
}

fn save_label(options: &SaveOptions) -> &'static str {
    if options.anti_cheat {
        "Sauvegardes : Anti-triche détecté : fichiers non séparés"
    } else if options.save_location == "profile" {
        "Sauvegardes : dans le profil du joueur"
    } else if options.save_isolation == "auto" {
        "Sauvegardes : séparées par joueur"
    } else {
        "Sauvegardes : fichiers non séparés"
    }
}

fn access_folder(game: &kinseat_ipc::GameEntry) -> &str {
    game.exe_path
        .as_deref()
        .filter(|path| !path.is_empty())
        .and_then(|path| path.rsplit_once(['\\', '/']).map(|(folder, _)| folder))
        .filter(|folder| !folder.is_empty())
        .unwrap_or(&game.install_path)
}

fn access_warning(status: Option<&str>) -> Option<(egui::Color32, &'static str)> {
    match status {
        Some("denied") => Some((
            t::WARN,
            "ne peut pas ouvrir ce jeu : il est dans un dossier auquel son compte n'a pas accès.",
        )),
        Some("missing") => Some((t::ERR, "fichier introuvable")),
        _ => None,
    }
}

fn suggested_name(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn executables(folder: &Path, depth: usize) -> Vec<PathBuf> {
    fn collect(folder: &Path, depth: usize, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(folder) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
            {
                found.push(path);
            } else if depth > 0 && path.is_dir() {
                collect(&path, depth - 1, found);
            }
        }
    }
    let mut found = Vec::new();
    collect(folder, depth, &mut found);
    found.sort();
    found
}

#[derive(Default)]
pub struct Panel {
    pub data: Option<Library>,
    pub options: HashMap<String, SaveOptions>,
    pub loading: bool,
    pub pending: Pending,
    pub error: Option<String>,
    pub search: String,
    pub source: String,
    pub show_add_form: bool,
    pub name: String,
    pub exe_path: String,
    /// Confirmation d'un octroi : jeu, membre, dossier exact envoyé au service.
    pub(crate) grant_confirm: Option<(String, String, String)>,
    pub(crate) picker: Option<std::sync::mpsc::Receiver<Option<std::path::PathBuf>>>,
    pub(crate) candidates: Vec<std::path::PathBuf>,
}
impl Panel {
    fn browse(&mut self, folder: bool, ctx: egui::Context) {
        if self.picker.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.picker = Some(rx);
        std::thread::spawn(move || {
            #[cfg(windows)]
            let result = if folder {
                rfd::FileDialog::new().pick_folder()
            } else {
                rfd::FileDialog::new()
                    .add_filter("Exécutable", &["exe"])
                    .pick_file()
            };
            #[cfg(not(windows))]
            let result: Option<std::path::PathBuf> = {
                let _ = folder;
                None
            };
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn choose(&mut self, path: std::path::PathBuf) {
        self.candidates.clear();
        if path.is_dir() {
            self.candidates = executables(&path, 2);
            if self.candidates.len() == 1 {
                self.choose(self.candidates[0].clone());
            } else if self.name.trim().is_empty() {
                self.name = suggested_name(&path);
            }
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        {
            self.exe_path = path.to_string_lossy().into_owned();
            if self.name.trim().is_empty() {
                self.name = suggested_name(&path);
            }
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        accounts: Option<&[Account]>,
        reduced: bool,
    ) -> Option<Command> {
        let mut action = None;
        if let Some(rx) = &self.picker {
            match rx.try_recv() {
                Ok(path) => {
                    self.picker = None;
                    if let Some(path) = path {
                        self.choose(path);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.picker = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => ui
                    .ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100)),
            }
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.loading, egui::Button::new("Analyser"))
                .clicked()
            {
                self.loading = true;
                action = Some(Command::LibraryScan);
            }
            let add_label = if self.show_add_form {
                "Masquer l'ajout manuel"
            } else {
                "+ Ajouter un jeu"
            };
            let add_btn = if self.show_add_form {
                egui::Button::new(t::text(add_label, 13.0, t::PRIMARY)).fill(t::HOVER)
            } else {
                egui::Button::new(t::text(add_label, 13.0, t::ON_ACCENT)).fill(t::ACCENT)
            };
            if ui.add(add_btn).clicked() {
                self.show_add_form = !self.show_add_form;
            }
            ui.label(t::text(
                "Catalogue local : Steam, Epic, GOG, EA, Ubisoft et jeux ajoutés manuellement.",
                13.0,
                t::SECONDARY,
            ));
        });
        ui.add_space(12.0);
        if let Some(error) = &self.error {
            ui.label(t::text(error, 14.0, t::ERR));
        }
        if self.loading {
            t::frame().show(ui, |ui| {
                ui.label(t::text(
                    "Analyse de la bibliothèque en cours…",
                    14.0,
                    t::SECONDARY,
                ));
                let progress = if reduced {
                    0.7
                } else {
                    ui.ctx()
                        .animate_value_with_time(ui.id().with("library-loading"), 0.7, 0.5)
                };
                ui.add(egui::ProgressBar::new(progress).desired_width(280.0));
            });
        }
        if self.show_add_form {
            t::frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(t::title("Ajouter un jeu manuellement", 16.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Fermer").clicked() {
                            self.show_add_form = false;
                        }
                    });
                });
                ui.add_space(6.0);
                ui.label("Nom du jeu");
                ui.add(
                    egui::TextEdit::singleline(&mut self.name)
                        .desired_width(350.0)
                        .hint_text("ex. Stardew Valley"),
                );
                ui.label("Chemin de l’exécutable (.exe)");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.exe_path)
                            .desired_width(340.0)
                            .hint_text(r"C:\Jeux\MonJeu\jeu.exe"),
                    );
                    if ui
                        .add_enabled(self.picker.is_none(), egui::Button::new("Parcourir…"))
                        .clicked()
                    {
                        self.browse(false, ui.ctx().clone());
                    }
                    if ui
                        .add_enabled(self.picker.is_none(), egui::Button::new("Dossier…"))
                        .clicked()
                    {
                        self.browse(true, ui.ctx().clone());
                    }
                });
                if !self.candidates.is_empty() {
                    ui.label("Choisir un exécutable du dossier :");
                    for path in self.candidates.clone() {
                        if ui.button(path.display().to_string()).clicked() {
                            self.choose(path);
                        }
                    }
                }
                if let Some(pending) = self.pending.get(ADD_KEY) {
                    progress(ui, pending);
                } else if ui
                    .add_enabled(
                        !self.loading && self.picker.is_none(),
                        egui::Button::new(t::text("Ajouter à la bibliothèque", 13.0, t::ON_ACCENT))
                            .fill(t::ACCENT),
                    )
                    .clicked()
                {
                    if self.exe_path.trim().is_empty() {
                        self.browse(false, ui.ctx().clone());
                    } else if !self.name.trim().is_empty()
                        && self.pending.begin(ADD_KEY, PendingAction::Add)
                    {
                        action = Some(Command::LibraryAdd {
                            name: self.name.trim().into(),
                            exe_path: self.exe_path.trim().into(),
                        });
                    }
                }
            });
            ui.add_space(12.0);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().interact_size.y = 32.0;
            ui.add_sized([80.0, 32.0], egui::Label::new("Rechercher"));
            ui.add_sized(
                [210.0, 32.0],
                egui::TextEdit::singleline(&mut self.search)
                    .vertical_align(egui::Align::Center)
                    .hint_text("Filtrer par nom…"),
            );
            egui::ComboBox::from_id_salt("library-source")
                .selected_text(if self.source.is_empty() {
                    "Toutes les sources"
                } else if self.source.eq_ignore_ascii_case("manual") {
                    "Manuel"
                } else {
                    &self.source
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.source, String::new(), "Toutes les sources");
                    if let Some(data) = &self.data {
                        let mut sources: Vec<_> =
                            data.games.iter().map(|g| g.source.as_str()).collect();
                        sources.sort_unstable();
                        sources.dedup();
                        for source in sources {
                            let label = if source.eq_ignore_ascii_case("manual") {
                                "Manuel"
                            } else {
                                source
                            };
                            ui.selectable_value(&mut self.source, source.into(), label);
                        }
                    }
                });
        });
        ui.add_space(12.0);
        let Some(data) = &self.data else {
            if !self.loading && self.error.is_none() {
                ui.label(t::text(
                    "Chargement de la bibliothèque…",
                    14.0,
                    t::SECONDARY,
                ));
            }
            return action;
        };
        if data.games.is_empty() {
            views::empty(
                ui,
                "Aucun jeu trouvé",
                "Lancez une analyse ou ajoutez le chemin d’un exécutable pour commencer.",
            );
        }
        let search = self.search.to_lowercase();
        let mut shown = 0;
        for game in &data.games {
            if (!self.source.is_empty() && self.source != game.source)
                || !game.name.to_lowercase().contains(&search)
            {
                continue;
            }
            shown += 1;
            t::frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.set_min_height(30.0);
                    ui.label(t::title(&game.name, 16.0));
                    let source_label = if game.source.eq_ignore_ascii_case("manual") {
                        "Manuel"
                    } else {
                        &game.source
                    };
                    views::badge(ui, source_label, t::ACCENT);
                    if let Some(pending) = self.pending.get(&game.id) {
                        progress(ui, pending);
                    } else if game.source.eq_ignore_ascii_case("manual") {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(egui::Button::new("Retirer").min_size(egui::vec2(76.0, 30.0)))
                                .clicked()
                                && self.pending.begin(&game.id, PendingAction::Remove)
                            {
                                action = Some(Command::LibraryRemove(game.id.clone()));
                            }
                        });
                    }
                });
                ui.label(t::text(&game.install_path, 12.0, t::FAINT));
                let options = self.options.get(&game.id).cloned().unwrap_or_else(|| SaveOptions { save_isolation: default_auto(), ..Default::default() });
                ui.label(t::text(save_label(&options), 13.0, t::SECONDARY));
                let mut from_host = options.start_from_host_save;
                if ui.add_enabled(!self.loading && self.pending.get(&game.id).is_none(),
                    egui::Checkbox::new(&mut from_host, "Partir de la sauvegarde de l'hôte"))
                    .changed() && self.pending.begin(&game.id, PendingAction::Update) {
                    action = Some(Command::LibrarySetOptions { game_id: game.id.clone(), start_from_host_save: from_host });
                }
                if let Some(store) = options.save_location.strip_prefix("cloud:").filter(|s| !s.is_empty()).or(options.store.as_deref().filter(|_| options.save_location.starts_with("cloud:"))) {
                    ui.label(t::text(format!("Si un joueur utilise le même compte {store} que vous, le cloud partage les parties."), 12.0, t::WARN));
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label(t::text("Autoriser pour :", 13.0, t::SECONDARY));
                    if let Some(accounts) = accounts {
                        for account in accounts.iter().filter(|a| a.is_managed) {
                            let mut allowed = game.assigned_to.contains(&account.username);
                            if ui
                                .add_enabled(
                                    !self.loading && self.pending.get(&game.id).is_none(),
                                    egui::Checkbox::new(&mut allowed, &account.username),
                                )
                                .changed()
                                && self.pending.begin(&game.id, PendingAction::Update)
                            {
                                action = Some(Command::LibraryAssign {
                                    game_id: game.id.clone(),
                                    account: account.username.clone(),
                                    allowed,
                                });
                            }
                            if allowed {
                                if let Some((color, message)) = access_warning(
                                    game.access.get(&account.username).map(String::as_str),
                                ) {
                                    ui.colored_label(color, format!("{} : {message}", account.username));
                                    if color == t::WARN {
                                        if ui.add_enabled(self.pending.get(&game.id).is_none(), egui::Button::new("Donner l'accès au dossier du jeu…")).clicked() {
                                            self.grant_confirm = Some((
                                                game.id.clone(),
                                                account.username.clone(),
                                                access_folder(game).to_owned(),
                                            ));
                                        }
                                        ui.label(t::text("ou déplacez le jeu, par exemple dans C:\\Jeux", 12.0, t::SECONDARY));
                                    }
                                }
                            }
                        }
                    } else {
                        ui.label("Chargement des membres…");
                    }
                });
                ui.label(t::text(
                    "Les jeux autorisés apparaissent sur le bureau du joueur et dans la liste des applications Moonlight.",
                    12.0,
                    t::SECONDARY,
                ));
            });
            ui.add_space(8.0);
        }
        if shown == 0 && !data.games.is_empty() {
            views::empty(
                ui,
                "Aucun résultat",
                "Essayez une autre recherche ou une autre source.",
            );
        }
        if let Some((game_id, account, folder)) = self.grant_confirm.clone() {
            let (mut cancel, mut confirm) = (false, false);
            let keys = t::modal_keys(ui.ctx(), egui::Id::new("Donner accès au dossier du jeu ?"), true);
            cancel |= keys.esc;
            let can_confirm = !self.loading && self.pending.get(&game_id).is_none();
            confirm |= keys.enter && can_confirm;
            egui::Window::new("Donner accès au dossier du jeu ?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label(format!(
                        "Donner à {account} l'accès en lecture et exécution au dossier :"
                    ));
                    ui.label(t::text(&folder, 13.0, t::PRIMARY));
                    ui.horizontal(|ui| {
                        if ui.button("Annuler").clicked() {
                            cancel = true;
                        }
                        if ui
                            .add_enabled(can_confirm, egui::Button::new("Donner l'accès"))
                            .clicked()
                        {
                            confirm = true;
                        }
                    });
                });
            if cancel || confirm {
                self.grant_confirm = None;
            }
            if confirm && self.pending.begin(&game_id, PendingAction::Update) {
                self.error = None;
                action = Some(Command::LibraryGrant { game_id, account });
            }
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_labels_and_legacy_defaults() {
        let legacy: SaveOptions = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(legacy.save_isolation, "auto");
        assert!(!legacy.start_from_host_save);
        assert_eq!(save_label(&legacy), "Sauvegardes : séparées par joueur");
        let profile: SaveOptions =
            serde_json::from_value(serde_json::json!({"saveLocation":"profile"})).unwrap();
        assert_eq!(
            save_label(&profile),
            "Sauvegardes : dans le profil du joueur"
        );
        let anti: SaveOptions =
            serde_json::from_value(serde_json::json!({"antiCheat":true,"saveLocation":"profile"}))
                .unwrap();
        assert_eq!(
            save_label(&anti),
            "Sauvegardes : Anti-triche détecté : fichiers non séparés"
        );
        let off: SaveOptions =
            serde_json::from_value(serde_json::json!({"saveIsolation":"off"})).unwrap();
        assert_eq!(save_label(&off), "Sauvegardes : fichiers non séparés");
        let (_, options) = parse_library(serde_json::json!({"games":[{"id":"g","name":"Jeu","source":"manual","installPath":"C:\\Jeux","assignedTo":[]}],"scannedAt":null})).unwrap();
        assert_eq!(options["g"].save_isolation, "auto");
    }

    #[test]
    fn pending_lifecycle() {
        let mut pending = Pending::default();
        assert!(pending.begin("game", PendingAction::Remove));
        assert!(!pending.begin("game", PendingAction::Update));
        assert_eq!(pending.get("game"), Some(PendingAction::Remove));
        pending.finish("game"); // succès : la carte sera retirée au rechargement
        assert_eq!(pending.get("game"), None);
        assert!(pending.begin("game", PendingAction::Update));
        pending.finish("game"); // erreur : la carte existante reste visible
        assert!(!pending.any());
        assert!(pending.begin(ADD_KEY, PendingAction::Add));
        assert!(!pending.begin(ADD_KEY, PendingAction::Add));
        pending.finish(ADD_KEY);
        assert!(!pending.any());
    }

    #[test]
    fn access_states_and_legacy_response() {
        assert_eq!(access_warning(None), None);
        assert_eq!(access_warning(Some("unknown")), None);
        assert_eq!(access_warning(Some("ok")), None);
        assert_eq!(access_warning(Some("denied")).unwrap().0, t::WARN);
        assert_eq!(access_warning(Some("missing")).unwrap().0, t::ERR);
        let legacy: kinseat_ipc::GameEntry = serde_json::from_value(serde_json::json!({
            "id":"game", "name":"Jeu", "source":"manual", "installPath":"C:\\Jeux\\Jeu",
            "exePath":"C:\\Jeux\\Jeu\\game.exe", "assignedTo":["Alice"]
        }))
        .unwrap();
        assert!(legacy.access.is_empty());
        assert_eq!(access_folder(&legacy), "C:\\Jeux\\Jeu");
    }

    #[test]
    fn executable_name_prefill() {
        assert_eq!(suggested_name(Path::new("Ma partie.exe")), "Ma partie");
        assert_eq!(suggested_name(Path::new("Mon jeu")), "Mon jeu");
    }
}
