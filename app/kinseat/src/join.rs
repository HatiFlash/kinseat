//! Demandes d'accès envoyées par les Clients (docs/join-requests.md §3).
//!
//! Le service expose `joinRequests.list`, `joinRequest.accept` et `joinRequest.refuse` sur le canal
//! d'administration. Ce module garde l'état de l'interface (choix de place, suivi de l'appairage)
//! sans toucher au réseau : toute la logique est testable sans service.

use crate::theme;
use kinseat_ipc::Seat;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequest {
    pub id: String,
    #[serde(default)]
    pub pseudo: String,
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub message: Option<String>,
}

/// Décode la réponse de `joinRequests.list` ; les entrées sans identifiant sont ignorées.
pub fn parse_list(value: serde_json::Value) -> Vec<JoinRequest> {
    let serde_json::Value::Array(items) = value else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| serde_json::from_value::<JoinRequest>(item).ok())
        .filter(|r| !r.id.is_empty())
        .collect()
}

/// Vrai si l'erreur signifie que le service ne connaît pas encore la méthode (ancienne version).
pub fn is_unknown_method(error: &str) -> bool {
    error.starts_with("unknown_method")
}

pub fn seat_label(seat: &Seat) -> &str {
    seat.display_name
        .as_deref()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or(&seat.account_name)
}

/// Places pouvant recevoir un appareil : état `Ready` uniquement.
pub fn ready_seats(seats: &[Seat]) -> Vec<&Seat> {
    seats.iter().filter(|s| s.status == "Ready").collect()
}

/// Message d'erreur en français pour une réponse du service.
pub fn error_text(error: &str) -> String {
    match error.split(':').next().unwrap_or("") {
        "conflict" => "Cette demande a déjà reçu une réponse, ou la place n'est plus prête.".into(),
        "not_found" => {
            "Cette demande n'existe plus (expirée ?) ou la place est introuvable.".into()
        }
        "invalid_params" => error
            .split_once(':')
            .map(|(_, detail)| detail.trim())
            .filter(|detail| !detail.is_empty())
            .unwrap_or("Le service a refusé cette réponse.")
            .into(),
        "unknown_method" => {
            "Cette version du service ne gère pas encore les demandes d'accès.".into()
        }
        _ => crate::notice::service_error(error),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    Pairing,
    Paired,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Tracked {
    pub pseudo: String,
    pub place: String,
    pub progress: Progress,
}
impl Tracked {
    pub fn text(&self) -> String {
        match &self.progress {
            Progress::Pairing => "Appairage en cours…".into(),
            Progress::Paired => format!("{} est associé à {}", self.pseudo, self.place),
            Progress::Failed(message) => message.clone(),
        }
    }
}

pub enum Action {
    Accept {
        request_id: String,
        seat_id: String,
        graphics: kinseat_ipc::GraphicsChoice,
    },
    CreateSeat {
        request_id: String,
        graphics: kinseat_ipc::GraphicsChoice,
    },
    Refuse(String),
    GoToSeats,
    Dismiss(String),
}

#[derive(Default)]
pub struct Panel {
    requests: Vec<JoinRequest>,
    /// Place choisie par demande (identifiant de demande → identifiant de place).
    choice: HashMap<String, String>,
    profiles: HashMap<String, (String, kinseat_ipc::GraphicsChoice)>,
    /// Demandes acceptées : suivi de l'appairage.
    tracked: Vec<(String, Tracked)>,
    /// Refus envoyés, en attente que la liste ne les montre plus.
    refusing: HashSet<String>,
    errors: HashMap<String, String>,
    /// « Aller aux places » : la fenêtre se replie jusqu'à ce qu'on la rouvre ou qu'une place soit prête.
    hidden: bool,
    /// Request shown first when it was hidden: a newer request pops up again.
    hidden_for: Option<String>,
    /// Ready seats seen at the previous frame: the hidden request comes back only when a seat
    /// becomes ready (e.g. created after « Aller aux places »), not on every frame.
    ready_before: usize,
}

impl Panel {
    pub fn apply_list(&mut self, list: Vec<JoinRequest>) {
        let pending: HashSet<&str> = list
            .iter()
            .filter(|r| r.status == "pending")
            .map(|r| r.id.as_str())
            .collect();
        self.refusing.retain(|id| pending.contains(id.as_str()));
        self.choice.retain(|id, _| pending.contains(id.as_str()));
        for (id, tracked) in &mut self.tracked {
            if let Some(request) = list.iter().find(|r| &r.id == id) {
                tracked.progress = match request.status.as_str() {
                    "paired" => Progress::Paired,
                    "failed" => Progress::Failed(
                        request
                            .message
                            .clone()
                            .unwrap_or_else(|| "L'appairage a échoué.".into()),
                    ),
                    "refused" => Progress::Failed("La demande a été refusée.".into()),
                    "expired" => Progress::Failed("La demande a expiré.".into()),
                    _ => tracked.progress.clone(),
                };
            }
        }
        self.requests = list;
    }

    /// Demandes à afficher dans la fenêtre : en attente, sans réponse déjà envoyée.
    pub fn pending(&self) -> Vec<&JoinRequest> {
        self.requests
            .iter()
            .filter(|r| {
                r.status == "pending"
                    && !self.refusing.contains(&r.id)
                    && !self.tracked.iter().any(|(id, _)| id == &r.id)
            })
            .collect()
    }

    /// Place proposée : le choix de l'utilisateur s'il est encore prêt, sinon la première place prête.
    pub fn effective_choice(&self, request_id: &str, seats: &[Seat]) -> Option<String> {
        let ready = ready_seats(seats);
        self.choice
            .get(request_id)
            .and_then(|id| ready.iter().find(|s| &s.id == id))
            .or_else(|| ready.first())
            .map(|s| s.id.clone())
    }

    pub fn accept(&mut self, request_id: &str, seat_id: &str, seats: &[Seat]) -> bool {
        let Some(request) = self.requests.iter().find(|r| r.id == request_id) else {
            return false;
        };
        let Some(seat) = seats
            .iter()
            .find(|s| s.id == seat_id && s.status == "Ready")
        else {
            self.errors
                .insert(request_id.into(), "Cette place n'est plus prête.".into());
            return false;
        };
        self.errors.remove(request_id);
        self.tracked.push((
            request_id.into(),
            Tracked {
                pseudo: request.pseudo.clone(),
                place: seat_label(seat).into(),
                progress: Progress::Pairing,
            },
        ));
        true
    }

    pub fn accept_created(&mut self, request_id: &str, seat: &Seat) -> bool {
        if !self.is_pending(request_id) {
            return false;
        }
        self.accept(request_id, &seat.id, std::slice::from_ref(seat))
    }

    pub fn is_pending(&self, request_id: &str) -> bool {
        self.requests
            .iter()
            .any(|r| r.id == request_id && r.status == "pending")
    }

    pub fn pseudo(&self, request_id: &str) -> Option<&str> {
        self.requests
            .iter()
            .find(|r| r.id == request_id)
            .map(|r| r.pseudo.as_str())
    }

    pub fn refuse(&mut self, request_id: &str) {
        self.errors.remove(request_id);
        self.refusing.insert(request_id.into());
    }

    /// Résultat de l'appel `joinRequest.accept` / `joinRequest.refuse`.
    pub fn answered(&mut self, request_id: &str, accepted: bool, error: Option<&str>) {
        let Some(error) = error else { return };
        let text = error_text(error);
        if accepted {
            self.tracked.retain(|(id, _)| id != request_id);
        } else {
            self.refusing.remove(request_id);
        }
        self.errors.insert(request_id.into(), text);
    }

    pub fn dismiss(&mut self, request_id: &str) {
        self.tracked.retain(|(id, _)| id != request_id);
    }

    #[cfg(test)]
    pub fn tracked(&self) -> &[(String, Tracked)] {
        &self.tracked
    }

    /// Démonstration : une demande fictive, avec le jeu de captures « Léa (PC-SALON, 192.168.0.42) ».
    pub fn demo_request(&mut self) {
        self.apply_list(vec![JoinRequest {
            id: "demo-join".into(),
            pseudo: "Léa".into(),
            device: "PC-SALON".into(),
            address: "192.168.0.42".into(),
            created_at: Some("2026-11-13T18:20:00Z".into()),
            status: "pending".into(),
            message: None,
        }]);
    }

    /// Démonstration : l'appareil est associé tout de suite.
    pub fn demo_set_status(&mut self, request_id: &str, status: &str) {
        let mut list = self.requests.clone();
        for r in list.iter_mut().filter(|r| r.id == request_id) {
            r.status = status.into();
        }
        self.apply_list(list);
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        seats: &[Seat],
        budget: Option<&kinseat_ipc::GpuBudget>,
        reduced: bool,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        let ready = ready_seats(seats);
        if ready.len() > self.ready_before {
            self.hidden = false;
        }
        self.ready_before = ready.len();
        let pending: Vec<JoinRequest> = self.pending().into_iter().cloned().collect();
        if self.hidden && pending.first().map(|r| &r.id) != self.hidden_for.as_ref() {
            self.hidden = pending.is_empty();
        }
        if let Some(request) = pending.first().filter(|_| !self.hidden) {
            let mut choice = self.effective_choice(&request.id, seats);
            let selected_seat = choice
                .as_ref()
                .and_then(|id| ready.iter().find(|s| &s.id == id));
            let initial = selected_seat
                .and_then(|seat| seat.graphics_profile.as_deref())
                .unwrap_or("Balanced");
            let (previous_seat, mut graphics) =
                self.profiles.get(&request.id).cloned().unwrap_or_else(|| {
                    (String::new(), kinseat_ipc::GraphicsChoice::preset(initial))
                });
            if choice.as_deref() != Some(previous_seat.as_str()) {
                graphics = kinseat_ipc::GraphicsChoice::preset(initial);
            }
            let screen = ctx.screen_rect();
            egui::Area::new(egui::Id::new("join-request-scrim"))
                .order(egui::Order::Middle)
                .fixed_pos(screen.min)
                .show(ctx, |ui| {
                    let (rect, _) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
                    ui.painter()
                        .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(145));
                });
            egui::Window::new("Demande d'accès")
                .id(egui::Id::new("join-request"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .max_width(480.0)
                .show(ctx, |ui| {
                    ui.set_width(440.0);
                    ui.label(theme::text(
                        format!(
                            "{} ({}, {}) demande à rejoindre ce PC",
                            request.pseudo, request.device, request.address
                        ),
                        15.0,
                        theme::PRIMARY,
                    ));
                    ui.add_space(12.0);
                    if ready.is_empty() {
                        ui.label(theme::text(
                            "Créez ou démarrez d'abord une place",
                            14.0,
                            theme::WARN,
                        ));
                    } else {
                        ui.label("Place");
                        let selected = choice
                            .as_ref()
                            .and_then(|id| ready.iter().find(|s| &s.id == id))
                            .map(|s| seat_label(s).to_owned())
                            .unwrap_or_default();
                        egui::ComboBox::from_id_salt("join-seat")
                            .selected_text(selected)
                            .width(330.0)
                            .show_ui(ui, |ui| {
                                for seat in &ready {
                                    ui.selectable_value(
                                        &mut choice,
                                        Some(seat.id.clone()),
                                        seat_label(seat),
                                    );
                                }
                            });
                        if let Some(id) = &choice {
                            if previous_seat != *id {
                                let profile = ready
                                    .iter()
                                    .find(|s| &s.id == id)
                                    .and_then(|s| s.graphics_profile.as_deref())
                                    .unwrap_or("Balanced");
                                graphics = kinseat_ipc::GraphicsChoice::preset(profile);
                            }
                            ui.add_space(8.0);
                            crate::graphics::selector(ui, "join-graphics", &mut graphics);
                            crate::graphics::gauge(ui, budget, &graphics, reduced);
                        }
                    }
                    if let Some(error) = self.errors.get(&request.id) {
                        ui.add_space(6.0);
                        ui.label(theme::text(error, 13.0, theme::ERR));
                    }
                    if pending.len() > 1 {
                        ui.add_space(6.0);
                        ui.label(theme::text(
                            format!("{} autre(s) demande(s) en attente.", pending.len() - 1),
                            13.0,
                            theme::SECONDARY,
                        ));
                    }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if let Some(seat_id) = choice.clone().filter(|_| !ready.is_empty()) {
                            if ui
                                .add(
                                    egui::Button::new(theme::text(
                                        "Accepter",
                                        14.0,
                                        theme::ON_ACCENT,
                                    ))
                                    .fill(theme::ACCENT),
                                )
                                .clicked()
                            {
                                actions.push(Action::Accept {
                                    request_id: request.id.clone(),
                                    seat_id,
                                    graphics: graphics.clone(),
                                });
                            }
                        }
                        let create_btn = if ready.is_empty() {
                            egui::Button::new(theme::text(
                                "Créer une place",
                                14.0,
                                theme::ON_ACCENT,
                            ))
                            .fill(theme::ACCENT)
                        } else {
                            egui::Button::new("Créer une place")
                        };
                        if ui.add(create_btn).clicked() {
                            actions.push(Action::CreateSeat {
                                request_id: request.id.clone(),
                                graphics: graphics.clone(),
                            });
                        }
                        if ui.button("Aller aux places").clicked() {
                            actions.push(Action::GoToSeats);
                        }
                        if ui.button("Refuser").clicked() {
                            actions.push(Action::Refuse(request.id.clone()));
                        }
                    });
                });
            if let Some(choice) = choice {
                self.profiles
                    .insert(request.id.clone(), (choice.clone(), graphics));
                self.choice.insert(request.id.clone(), choice);
            }
        } else if let Some(first) = pending.first() {
            egui::Window::new("Demande d'accès en attente")
                .id(egui::Id::new("join-request-banner"))
                .title_bar(false)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::RIGHT_TOP, [-16.0, 16.0])
                .show(ctx, |ui| {
                    ui.label(theme::text(
                        format!("{} attend une réponse.", first.pseudo),
                        13.0,
                        theme::SECONDARY,
                    ));
                    if ui.button("Afficher la demande").clicked() {
                        self.hidden = false;
                    }
                });
        }
        if !self.tracked.is_empty() {
            egui::Window::new("Association d'appareil")
                .id(egui::Id::new("join-progress"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -16.0])
                .show(ctx, |ui| {
                    ui.set_width(340.0);
                    for (id, tracked) in &self.tracked {
                        let color = match tracked.progress {
                            Progress::Pairing => theme::SECONDARY,
                            Progress::Paired => theme::OK,
                            Progress::Failed(_) => theme::ERR,
                        };
                        ui.horizontal(|ui| {
                            ui.label(theme::text(tracked.text(), 14.0, color));
                            if tracked.progress != Progress::Pairing
                                && ui.small_button("Fermer").clicked()
                            {
                                actions.push(Action::Dismiss(id.clone()));
                            }
                        });
                    }
                });
        }
        if actions.iter().any(|a| matches!(a, Action::GoToSeats)) {
            self.hidden = true;
            self.hidden_for = pending.first().map(|r| r.id.clone());
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seat(id: &str, name: &str, status: &str, display: Option<&str>) -> Seat {
        serde_json::from_value(serde_json::json!({
            "id": id, "accountName": name, "displayName": display, "status": status,
            "sessionId": 1, "width": 1920, "height": 1080
        }))
        .unwrap()
    }
    fn list() -> Vec<JoinRequest> {
        parse_list(serde_json::json!([
            {"id":"r1","pseudo":"Léa","device":"PC-SALON","address":"192.168.0.42","createdAt":"2026-11-13T18:20:00Z","status":"pending"},
            {"id":"r2","pseudo":"Noé","device":"PC","address":"192.168.0.43","status":"accepted"},
            {"pseudo":"sans id"}
        ]))
    }

    #[test]
    fn parses_list_and_skips_invalid_entries() {
        let l = list();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].pseudo, "Léa");
        assert_eq!(l[0].address, "192.168.0.42");
        assert_eq!(l[1].status, "accepted");
        assert!(parse_list(serde_json::json!({"x":1})).is_empty());
    }

    #[test]
    fn only_pending_requests_are_listed() {
        let mut p = Panel::default();
        p.apply_list(list());
        let pending = p.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "r1");
    }

    #[test]
    fn default_seat_is_first_ready_and_choice_is_kept() {
        let seats = vec![
            seat("1", "a", "Streaming", None),
            seat("2", "b", "Ready", Some("Salon")),
            seat("3", "c", "Ready", None),
        ];
        let mut p = Panel::default();
        p.apply_list(list());
        assert_eq!(p.effective_choice("r1", &seats).as_deref(), Some("2"));
        p.choice.insert("r1".into(), "3".into());
        assert_eq!(p.effective_choice("r1", &seats).as_deref(), Some("3"));
        // La place choisie n'est plus prête : retour à la première place prête.
        let seats = vec![seat("2", "b", "Ready", None), seat("3", "c", "Idle", None)];
        assert_eq!(p.effective_choice("r1", &seats).as_deref(), Some("2"));
        assert_eq!(
            p.effective_choice("r1", &[seat("3", "c", "Idle", None)]),
            None
        );
        assert_eq!(seat_label(&seat("2", "b", "Ready", Some("Salon"))), "Salon");
        assert_eq!(seat_label(&seat("2", "b", "Ready", Some(" "))), "b");
    }

    #[test]
    fn invalid_params_keeps_service_detail() {
        assert_eq!(
            error_text("invalid_params: largeur incorrecte"),
            "largeur incorrecte"
        );
    }

    #[test]
    fn created_seat_is_accepted_only_when_ready() {
        let mut panel = Panel::default();
        panel.apply_list(list());
        assert!(!panel.accept_created("r1", &seat("new", "lea", "Provisioning", None)));
        assert_eq!(panel.pending().len(), 1);
        assert!(panel.accept_created("r1", &seat("new", "lea", "Ready", None)));
        assert!(panel.pending().is_empty());
        assert_eq!(panel.tracked()[0].1.place, "lea");
    }

    #[test]
    fn accept_tracks_pairing_then_paired() {
        let seats = vec![seat("2", "b", "Ready", Some("Salon"))];
        let mut p = Panel::default();
        p.apply_list(list());
        assert!(p.accept("r1", "2", &seats));
        assert!(p.pending().is_empty());
        assert_eq!(p.tracked()[0].1.text(), "Appairage en cours…");
        let mut l = list();
        l[0].status = "accepted".into();
        p.apply_list(l.clone());
        assert_eq!(p.tracked()[0].1.progress, Progress::Pairing);
        l[0].status = "paired".into();
        p.apply_list(l.clone());
        assert_eq!(p.tracked()[0].1.text(), "Léa est associé à Salon");
        l[0].status = "failed".into();
        l[0].message = Some("PIN incorrect".into());
        p.apply_list(l);
        assert_eq!(p.tracked()[0].1.text(), "PIN incorrect");
        p.dismiss("r1");
        assert!(p.tracked().is_empty());
    }

    #[test]
    fn accept_refused_by_service_returns_to_dialog_with_error() {
        let seats = vec![seat("2", "b", "Ready", None)];
        let mut p = Panel::default();
        p.apply_list(list());
        assert!(p.accept("r1", "2", &seats));
        p.answered("r1", true, Some("conflict: place occupée"));
        assert_eq!(p.pending().len(), 1);
        assert!(p.tracked().is_empty());
        assert!(p.errors["r1"].contains("déjà reçu une réponse"));
        // Place non prête : aucun envoi.
        assert!(!p.accept("r1", "9", &seats));
    }

    #[test]
    fn refuse_hides_request_until_service_confirms_or_fails() {
        let mut p = Panel::default();
        p.apply_list(list());
        p.refuse("r1");
        assert!(p.pending().is_empty());
        p.answered("r1", false, Some("not_found"));
        assert_eq!(p.pending().len(), 1);
        p.refuse("r1");
        let mut l = list();
        l[0].status = "refused".into();
        p.apply_list(l);
        assert!(p.pending().is_empty());
        assert!(p.refusing.is_empty());
    }

    #[test]
    fn unknown_method_is_detected() {
        assert!(is_unknown_method("unknown_method: Méthode inconnue"));
        assert!(!is_unknown_method("forbidden: Acces"));
    }
}
