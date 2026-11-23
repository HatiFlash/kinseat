use crate::{
    action, card,
    discovery::{self, Place},
    engine::{self, Engine, Event},
    fixed_card, footer, header,
    join::{HostInfo, JoinParams, Phase},
    picker, profile, radar, section, space, text, theme as t, title, toggle, Client, Screen,
};

/// Champ de saisie d'un écran de profil (étiquette, aide, masque pour les codes).
fn field(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    hint: &str,
    secret: bool,
    limit: usize,
) -> egui::Response {
    text(ui, label, 14.0, t::SECONDARY);
    space(ui, 4.0);
    let response = ui.add_sized(
        [360.0, 36.0],
        egui::TextEdit::singleline(value)
            .password(secret)
            .hint_text(hint)
            .char_limit(limit),
    );
    space(ui, 10.0);
    response
}
fn entered(ui: &egui::Ui, response: &egui::Response) -> bool {
    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}
fn feedback(ui: &mut egui::Ui, message: &Option<(bool, String)>) {
    if let Some((ok, message)) = message {
        text(ui, message, 13.0, if *ok { t::OK } else { t::ERR });
        space(ui, 8.0);
    }
}

impl Client {
    fn engine(&self) -> Option<Engine> {
        Engine::locate(&self.settings.engine_path)
    }
    /// Envoie la demande d'accès à l'Hôte ; l'appairage ne démarre qu'après son accord.
    fn request_access(&mut self, host: HostInfo) {
        if self.demo {
            self.notice = Some("Indisponible en démonstration".into());
            return;
        }
        if self.join.is_active() {
            return;
        }
        let Some(engine) = self.engine() else {
            self.notice =
                Some("Moteur Moonlight introuvable : indiquez son chemin dans Paramètres.".into());
            return;
        };
        if profile::ensure_device_id(&mut self.settings) {
            self.persist();
        }
        let params = JoinParams {
            pseudo: self.settings.pseudo.clone(),
            device: profile::device_name(&self.settings),
            device_id: self.settings.device_id.clone(),
        };
        self.notice = None;
        self.join.start(host, params, engine);
        self.screen = Screen::Waiting;
    }
    /// Pose un état d'aperçu (PC fictif, aucun appel réseau) pour capturer un écran réel.
    pub fn preview_real(&mut self, screen: Screen) {
        let host = HostInfo {
            name: "KINSEAT-HOST".into(),
            address: "192.168.1.20".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        };
        self.selected = Some(Place {
            name: host.name.clone(),
            address: host.address.clone(),
            port: 48100,
            session_access: None,
        });
        if self.settings.pseudo.is_empty() {
            self.settings.pseudo = "Yanis".into();
        }
        if screen == Screen::Profile {
            self.form.pseudo = "Yanis".into();
        }
        if screen == Screen::Waiting {
            self.join.phase = Some(Phase::Waiting);
            self.join.host = Some(host.clone());
            self.join.hosts.push(host);
        }
        self.return_to = Screen::Home;
        self.screen = screen;
    }
    fn start_stream(&mut self) {
        let (Some(engine), Some(place)) = (self.engine(), self.selected.clone()) else {
            self.notice = Some("Place ou moteur Moonlight indisponible.".into());
            return;
        };
        let args = crate::engine::stream_args(
            &place.host(),
            &self.settings.app_name,
            &self.settings,
            (self.device.screen.width, self.device.screen.height),
            self.device.screen.hz,
        );
        self.launching = true;
        self.notice = None;
        self.stop_tx = Some(crate::engine::stream(
            engine,
            place,
            args,
            self.events_tx.clone(),
        ));
    }
    fn poll(&mut self) {
        while let Ok(place) = self.found.try_recv() {
            discovery::merge(&mut self.places, place);
        }
        self.join.schedule(&self.places);
        if let Some(notice) = self.join.poll_probes() {
            self.notice = Some(notice);
        }
        if let Some(place) = self.join.poll_phases() {
            self.settings.linked_place = Some(place.clone());
            self.selected = Some(place);
            self.notice = None;
            self.persist();
            self.screen = Screen::Home;
        }
        while let Ok(event) = self.events_rx.try_recv() {
            match event {
                // L'appairage est suivi par le flux de demande d'accès (join.rs).
                Event::Pair(_) => {}
                Event::Started(pid) => {
                    self.moonlight_pid = Some(pid);
                    self.stream_started = Some(std::time::Instant::now());
                    self.start_probe();
                    self.launching = false;
                    self.playing = true;
                    self.notice = None;
                }
                Event::Terminated(message) => {
                    self.stop_panel();
                    self.launching = false;
                    self.playing = false;
                    self.stop_tx = None;
                    self.screen = Screen::Home;
                    self.notice = Some(message);
                }
                Event::Stopped(message) => {
                    self.stop_panel();
                    self.launching = false;
                    self.playing = false;
                    self.stop_tx = None;
                    self.notice = Some(message);
                }
            }
        }
    }
    pub fn real_update(&mut self, ctx: &egui::Context) {
        self.poll();
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        let locked = matches!(self.screen, Screen::Profile | Screen::Lock);
        if header(ctx, self.screen == Screen::Settings, None) && !locked {
            if self.screen == Screen::Settings {
                self.screen = self.return_to;
            } else {
                self.return_to = self.screen;
                self.screen = Screen::Settings;
            }
        }
        footer(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t::CANVAS))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("page")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let width = (ui.available_width() - 80.0).clamp(640.0, 1040.0);
                        let margin = (ui.available_width() - width).max(0.0) / 2.0;
                        ui.add_space(30.0);
                        ui.horizontal(|ui| {
                            ui.add_space(margin);
                            ui.vertical(|ui| {
                                ui.set_width(width);
                                match self.screen {
                                    Screen::Search => self.real_search(ui),
                                    Screen::Waiting => self.waiting_ui(ui),
                                    Screen::Home => self.real_home(ui),
                                    Screen::Settings => self.real_settings(ui),
                                    Screen::Profile => self.profile_ui(ui),
                                    Screen::Lock => self.lock_ui(ui),
                                    _ => self.screen = Screen::Search,
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
    }
    fn real_search(&mut self, ui: &mut egui::Ui) {
        text(ui, "01  /  TROUVER LE PC", 12.0, t::ACCENT);
        space(ui, 12.0);
        title(ui, "Votre PC Kinseat");
        text(
            ui,
            format!(
                "Salut {} ! Choisis un PC et demande l'accès : il devra l'accepter.",
                self.settings.pseudo
            ),
            15.0,
            t::SECONDARY,
        );
        space(ui, 24.0);
        let col = ((ui.available_width() - 24.0) / 2.0).clamp(300.0, 430.0);
        ui.horizontal_top(|ui| {
            radar(ui, col, !self.join.hosts.is_empty());
            ui.add_space(24.0);
            ui.vertical(|ui| {
                ui.set_width(col);
                section(ui, "PC disponibles");
                space(ui, 16.0);
                if self.join.hosts.is_empty() {
                    card(ui, |ui| {
                        ui.set_width(col - 50.0);
                        section(ui, "Recherche en cours…");
                        text(
                            ui,
                            "Recherche des PC Kinseat sur le réseau local.",
                            14.0,
                            t::SECONDARY,
                        );
                        space(ui, 8.0);
                        text(
                            ui,
                            "Le PC hôte doit être allumé, avec Kinseat Hôte démarré.",
                            13.0,
                            t::FAINT,
                        );
                    });
                }
                for host in self.join.hosts.clone() {
                    card(ui, |ui| {
                        ui.set_width(col - 50.0);
                        section(ui, &host.name);
                        text(
                            ui,
                            format!("{}  ·  Réseau local", host.address),
                            14.0,
                            t::SECONDARY,
                        );
                        space(ui, 12.0);
                        text(ui, "Accepte les demandes d'accès", 14.0, t::OK);
                        space(ui, 20.0);
                        if action(ui, "Demander l'accès", true) {
                            self.request_access(host.clone());
                        }
                    });
                    space(ui, 12.0);
                }
                space(ui, 8.0);
                if !self.manual_entry {
                    if action(ui, "Saisir une adresse", false) {
                        self.manual_entry = true;
                    }
                } else {
                    card(ui, |ui| {
                        ui.set_width(col - 50.0);
                        section(ui, "Adresse du PC");
                        text(ui, "Ex. 192.168.1.20", 13.0, t::SECONDARY);
                        space(ui, 10.0);
                        let edit = ui.add_sized(
                            [col - 50.0, 36.0],
                            egui::TextEdit::singleline(&mut self.address)
                                .hint_text("Adresse IP ou nom du PC"),
                        );
                        let enter = entered(ui, &edit);
                        space(ui, 12.0);
                        ui.horizontal(|ui| {
                            let label = if self.join.manual_pending {
                                "Recherche…"
                            } else {
                                "Rechercher ce PC"
                            };
                            if (action(ui, label, true) || enter) && !self.join.manual_pending {
                                match discovery::parse_address(&self.address) {
                                    Ok(p) => self.join.probe_manual(p.address),
                                    Err(e) => self.notice = Some(e.into()),
                                }
                            }
                            if action(ui, "Annuler", false) {
                                self.manual_entry = false;
                            }
                        });
                    });
                }
            });
        });
    }
    /// Attente de l'accord de l'Hôte, puis appairage (écran réel et démonstration).
    pub fn waiting_ui(&mut self, ui: &mut egui::Ui) {
        let (Some(host), Some(phase)) = (self.join.host.clone(), self.join.phase.clone()) else {
            self.screen = Screen::Search;
            return;
        };
        text(ui, "02  /  DEMANDE D'ACCÈS", 12.0, t::ACCENT);
        space(ui, 12.0);
        let (heading, detail, color) = match &phase {
            Phase::Sending | Phase::Waiting => (
                format!("Demande envoyée à {}", host.name),
                "En attente de son accord. Quelqu'un doit cliquer sur Accepter sur le PC hôte."
                    .to_owned(),
                t::SECONDARY,
            ),
            Phase::Pairing => (
                format!("Demande acceptée par {}", host.name),
                "Accord reçu : association de cet appareil en cours…".to_owned(),
                t::OK,
            ),
            Phase::Ready(_) => (
                "Prêt à jouer".to_owned(),
                "Association terminée.".to_owned(),
                t::OK,
            ),
            Phase::Refused => (
                "Demande refusée".to_owned(),
                format!("{} a refusé la demande.", host.name),
                t::ERR,
            ),
            Phase::Expired => (
                "Demande expirée".to_owned(),
                "La demande a expiré.".to_owned(),
                t::ERR,
            ),
            Phase::Failed(message) => ("Demande interrompue".to_owned(), message.clone(), t::ERR),
            Phase::Cancelled => {
                self.screen = Screen::Search;
                return;
            }
        };
        title(ui, &heading);
        space(ui, 24.0);
        let card_w = 580.0_f32.min(ui.available_width());
        let left_pad = ((ui.available_width() - card_w) / 2.0).max(0.0);
        ui.horizontal(|ui| {
            ui.add_space(left_pad);
            fixed_card(ui, card_w, |ui| {
                ui.set_width((card_w - 50.0).max(240.0));
                // Animation visuelle d'appairage (Client <-> PC hôte).
                if phase.is_active() {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(33));
                }
                let time = ui.ctx().input(|i| i.time) as f32;
                let (anim_rect, _) = ui.allocate_exact_size(
                    egui::vec2((card_w - 50.0).max(240.0), 108.0),
                    egui::Sense::hover(),
                );
                let p = ui.painter();
                let cy = anim_rect.center().y;
                let left_dev = egui::Rect::from_center_size(
                    egui::pos2(anim_rect.center().x - 110.0, cy),
                    egui::vec2(54.0, 38.0),
                );
                let right_dev = egui::Rect::from_center_size(
                    egui::pos2(anim_rect.center().x + 110.0, cy),
                    egui::vec2(64.0, 44.0),
                );
                let sig_color = match &phase {
                    Phase::Pairing | Phase::Ready(_) => t::OK,
                    Phase::Sending | Phase::Waiting => t::ACCENT,
                    _ => t::ERR,
                };
                // Piste de liaison entre les deux appareils.
                let line_start = egui::pos2(left_dev.right() + 10.0, cy);
                let line_end = egui::pos2(right_dev.left() - 10.0, cy);
                p.line_segment(
                    [line_start, line_end],
                    egui::Stroke::new(2.0_f32, t::BORDER),
                );
                if phase.is_active() {
                    // Trois impulsions lumineuses qui circulent sur la liaison.
                    for k in 0..3 {
                        let frac = (time * 0.75 + k as f32 / 3.0).fract();
                        let x = line_start.x + (line_end.x - line_start.x) * frac;
                        let glow = (1.0 - (frac - 0.5).abs() * 2.0).clamp(0.15, 1.0);
                        p.circle_filled(
                            egui::pos2(x, cy),
                            6.5,
                            sig_color.gamma_multiply(glow * 0.25),
                        );
                        p.circle_filled(egui::pos2(x, cy), 3.2, sig_color.gamma_multiply(glow));
                    }
                    // Halo pulsant autour du PC hôte.
                    let pulse = (time * 1.4).fract();
                    p.circle_stroke(
                        right_dev.center(),
                        30.0 + pulse * 18.0,
                        egui::Stroke::new(1.5_f32, sig_color.gamma_multiply((1.0 - pulse) * 0.55)),
                    );
                }
                p.rect_filled(left_dev, 6, t::CANVAS);
                p.rect_stroke(
                    left_dev,
                    6,
                    egui::Stroke::new(2.0_f32, t::SECONDARY),
                    egui::StrokeKind::Inside,
                );
                p.rect_filled(right_dev, 7, t::CANVAS);
                p.rect_stroke(
                    right_dev,
                    7,
                    egui::Stroke::new(2.5_f32, sig_color),
                    egui::StrokeKind::Inside,
                );
                p.line_segment(
                    [
                        egui::pos2(right_dev.center().x - 16.0, right_dev.bottom() + 6.0),
                        egui::pos2(right_dev.center().x + 16.0, right_dev.bottom() + 6.0),
                    ],
                    egui::Stroke::new(2.0_f32, sig_color),
                );
                space(ui, 8.0);
                ui.vertical_centered(|ui| {
                    section(
                        ui,
                        match &phase {
                            Phase::Sending => "Envoi de la demande…",
                            Phase::Waiting => "En attente de l'accord…",
                            Phase::Pairing => "Association en cours…",
                            Phase::Ready(_) => "Appareil associé",
                            _ => "Demande terminée",
                        },
                    );
                    space(ui, 6.0);
                    ui.add(
                        egui::Label::new(egui::RichText::new(detail).size(14.0).color(color))
                            .wrap(),
                    );
                    space(ui, 8.0);
                    text(
                        ui,
                        format!("PC : {}  ·  {}", host.name, host.address),
                        12.5,
                        t::FAINT,
                    );
                });
                space(ui, 20.0);
                ui.horizontal(|ui| {
                    if phase.is_active() {
                        if action(ui, "Annuler", false) {
                            self.join.cancel();
                            self.screen = Screen::Search;
                        }
                    } else {
                        if action(ui, "Réessayer", true) {
                            self.request_access(host.clone());
                        }
                        if action(ui, "Retour", false) {
                            self.join.cancel();
                            self.screen = Screen::Search;
                        }
                    }
                });
            });
        });
    }
    /// Création du profil au premier lancement (écran réel et démonstration).
    pub fn profile_ui(&mut self, ui: &mut egui::Ui) {
        text(ui, "BIENVENUE", 12.0, t::ACCENT);
        space(ui, 12.0);
        title(ui, "Créer ton profil");
        text(
            ui,
            "Le pseudo est montré au PC hôte quand tu demandes l'accès.",
            15.0,
            t::SECONDARY,
        );
        space(ui, 20.0);
        let mut submit = false;
        let card_w = 460.0_f32.min(ui.available_width());
        let left_pad = ((ui.available_width() - card_w) / 2.0).max(0.0);
        ui.horizontal(|ui| {
            ui.add_space(left_pad);
            fixed_card(ui, card_w, |ui| {
                ui.set_width((card_w - 50.0).max(240.0));
                field(
                    ui,
                    "Pseudo (obligatoire)",
                    &mut self.form.pseudo,
                    "Ton pseudo",
                    false,
                    24,
                );
                text(
                    ui,
                    "Code PIN facultatif : demandé à chaque ouverture de Kinseat.",
                    13.0,
                    t::FAINT,
                );
                space(ui, 8.0);
                field(
                    ui,
                    "Code PIN (4 à 8 chiffres)",
                    &mut self.form.pin,
                    "Laisser vide : aucun code",
                    true,
                    8,
                );
                let confirm = field(
                    ui,
                    "Confirmer le code PIN",
                    &mut self.form.confirm,
                    "Retape le code",
                    true,
                    8,
                );
                submit = entered(ui, &confirm);
                feedback(ui, &self.form.message);
                if action(ui, "Créer mon profil", true) {
                    submit = true;
                }
            });
        });
        if submit {
            self.submit_profile();
        }
    }
    fn submit_profile(&mut self) {
        if self.demo {
            self.notice = Some("Indisponible en démonstration".into());
            return;
        }
        match profile::create_profile(
            &mut self.settings,
            &self.form.pseudo,
            &self.form.pin,
            &self.form.confirm,
        ) {
            Ok(()) => {
                self.form.clear_pins();
                self.form.message = None;
                self.notice = None;
                self.persist();
                self.screen = self.after_lock;
            }
            Err(e) => self.form.message = Some((false, e)),
        }
    }
    /// Code PIN demandé à l'ouverture quand il est défini.
    pub fn lock_ui(&mut self, ui: &mut egui::Ui) {
        text(ui, "CLIENT VERROUILLÉ", 12.0, t::ACCENT);
        space(ui, 12.0);
        let pseudo = if self.settings.pseudo.is_empty() {
            "joueur".to_owned()
        } else {
            self.settings.pseudo.clone()
        };
        title(ui, &format!("Bonjour {pseudo}"));
        text(
            ui,
            "Saisis ton code PIN pour ouvrir Kinseat.",
            15.0,
            t::SECONDARY,
        );
        space(ui, 20.0);
        let mut submit = false;
        let card_w = 460.0_f32.min(ui.available_width());
        let left_pad = ((ui.available_width() - card_w) / 2.0).max(0.0);
        ui.horizontal(|ui| {
            ui.add_space(left_pad);
            fixed_card(ui, card_w, |ui| {
                ui.set_width((card_w - 50.0).max(240.0));
                let edit = field(ui, "Code PIN", &mut self.form.current, "••••", true, 8);
                if !edit.has_focus() {
                    edit.request_focus();
                }
                submit = entered(ui, &edit);
                feedback(ui, &self.form.message);
                if action(ui, "Ouvrir", true) {
                    submit = true;
                }
            });
        });
        if submit {
            if self.demo {
                self.notice = Some("Indisponible en démonstration".into());
            } else if profile::check_pin(&self.settings, &self.form.current) {
                self.form.clear_pins();
                self.form.message = None;
                self.screen = self.after_lock;
            } else {
                self.form.current.clear();
                self.form.message = Some((false, "Code PIN incorrect.".into()));
            }
        }
    }
    /// Carte « Profil » des Paramètres : pseudo, définir / changer / retirer le PIN.
    fn profile_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            section(ui, "Profil");
            space(ui, 8.0);
            field(ui, "Pseudo", &mut self.form.pseudo, "Ton pseudo", false, 24);
            if action(ui, "Enregistrer le pseudo", false) {
                self.form.message =
                    match profile::change_pseudo(&mut self.settings, &self.form.pseudo) {
                        Ok(()) => Some((true, "Pseudo enregistré.".into())),
                        Err(e) => Some((false, e)),
                    };
            }
            space(ui, 14.0);
            let has_pin = profile::has_pin(&self.settings);
            text(
                ui,
                if has_pin {
                    "Code PIN : défini (demandé à l'ouverture)"
                } else {
                    "Code PIN : aucun"
                },
                14.0,
                t::SECONDARY,
            );
            space(ui, 8.0);
            if has_pin {
                field(
                    ui,
                    "Code PIN actuel",
                    &mut self.form.current,
                    "Code actuel",
                    true,
                    8,
                );
            }
            field(
                ui,
                "Nouveau code PIN (4 à 8 chiffres)",
                &mut self.form.pin,
                "Nouveau code",
                true,
                8,
            );
            field(
                ui,
                "Confirmer le nouveau code",
                &mut self.form.confirm,
                "Retape le code",
                true,
                8,
            );
            ui.horizontal(|ui| {
                if action(
                    ui,
                    if has_pin {
                        "Changer le code"
                    } else {
                        "Définir le code"
                    },
                    false,
                ) {
                    let result = profile::change_pin(
                        &mut self.settings,
                        &self.form.current,
                        &self.form.pin,
                        &self.form.confirm,
                    );
                    self.form.message = Some(match result {
                        Ok(()) => {
                            self.form.clear_pins();
                            (true, "Code PIN enregistré.".into())
                        }
                        Err(e) => (false, e),
                    });
                }
                if has_pin && action(ui, "Retirer le code", false) {
                    self.form.message = Some(
                        match profile::remove_pin(&mut self.settings, &self.form.current) {
                            Ok(()) => {
                                self.form.clear_pins();
                                (true, "Code PIN retiré.".into())
                            }
                            Err(e) => (false, e),
                        },
                    );
                }
            });
            space(ui, 8.0);
            feedback(ui, &self.form.message);
        });
    }
    fn real_home(&mut self, ui: &mut egui::Ui) {
        let Some(place) = self.selected.clone() else {
            self.screen = Screen::Search;
            return;
        };
        text(ui, "03  /  VOTRE PLACE", 12.0, t::ACCENT);
        space(ui, 12.0);
        title(ui, "Prêt à jouer");
        text(
            ui,
            format!("{}  ·  {}  ·  lié", place.name, place.address),
            14.0,
            t::SECONDARY,
        );
        space(ui, 18.0);
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            section(ui, "Votre session");
            space(ui, 8.0);
            let (status, color) = if self.playing {
                ("En cours sur votre place", t::OK)
            } else if self.launching {
                ("Démarrage de votre place…", t::ACCENT)
            } else {
                ("Disponible pour jouer", t::OK)
            };
            text(ui, status, 20.0, color);
            space(ui, 18.0);
            if self.playing || self.launching {
                if action(ui, "Arrêter", false) {
                    if let Some(tx) = self.stop_tx.take() {
                        let _ = tx.send(());
                    }
                }
            } else if action(ui, "Jouer", true) {
                self.start_stream();
            }
        });
        space(ui, 16.0);
        let half = ((ui.available_width() - 16.0) / 2.0).clamp(300.0, 520.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            fixed_card(ui, half, |ui| {
                ui.set_width(half - 48.0);
                section(ui, "Écran");
                space(ui, 12.0);
                text(ui, self.device.screen_label(), 14.0, t::PRIMARY);
                text(
                    ui,
                    format!("Application lancée : {}", self.settings.app_name),
                    14.0,
                    t::SECONDARY,
                );
            });
            ui.add_space(16.0);
            fixed_card(ui, half, |ui| {
                ui.set_width(half - 48.0);
                section(ui, "Moteur de jeu à distance");
                space(ui, 12.0);
                match self.engine() {
                    Some(_) => text(ui, "Moonlight prêt", 14.0, t::OK),
                    None => text(ui, "Moonlight introuvable : voir Paramètres", 14.0, t::ERR),
                }
                text(
                    ui,
                    "Temps de jeu et historique : bientôt fournis par le PC hôte.",
                    13.0,
                    t::FAINT,
                );
            });
        });
    }
    fn real_settings(&mut self, ui: &mut egui::Ui) {
        text(ui, "PARAMÈTRES", 12.0, t::ACCENT);
        space(ui, 12.0);
        title(ui, "Paramètres");
        space(ui, 18.0);
        let before = self.settings.clone();
        ui.vertical(|ui| {
            self.profile_card(ui);
            space(ui, 12.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                section(ui, "Vidéo");
                text(
                    ui,
                    format!(
                        "Automatique : écran {} × {} (fréquence non mesurée)",
                        self.device.screen.width, self.device.screen.height
                    ),
                    13.0,
                    t::SECONDARY,
                );
                let mut options = vec!["Automatique".to_owned(), "Personnalisée".to_owned()];
                options.extend(
                    crate::recommend::same_ratio_options(self.device.screen)
                        .iter()
                        .map(|(w, h)| format!("{w} × {h}")),
                );
                picker(
                    ui,
                    "Résolution",
                    &mut self.settings.resolution,
                    &options.iter().map(String::as_str).collect::<Vec<_>>(),
                );
                if self.settings.resolution == "Personnalisée" {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.settings.custom_width).range(640..=7680),
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
                    &["Automatique", "30", "60", "90", "120", "144"],
                );
                toggle(
                    ui,
                    "Débit automatique (Moonlight)",
                    &mut self.settings.automatic_bitrate,
                );
                if !self.settings.automatic_bitrate {
                    ui.add(egui::Slider::new(&mut self.settings.bitrate, 1..=150).text("Mb/s"));
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
                for (name, val) in [
                    ("HDR", &mut self.settings.hdr),
                    ("YUV 4:4:4", &mut self.settings.yuv444),
                    ("V-Sync", &mut self.settings.vsync),
                    ("Frame pacing", &mut self.settings.frame_pacing),
                ] {
                    toggle(ui, name, val);
                }
            });
            space(ui, 12.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                section(ui, "Affichage & audio");
                picker(
                    ui,
                    "Mode",
                    &mut self.settings.display_mode,
                    &["Plein écran", "Fenêtré sans bordure", "Fenêtré"],
                );
                picker(
                    ui,
                    "Audio",
                    &mut self.settings.audio,
                    &["Stéréo", "5.1", "7.1"],
                );
                toggle(
                    ui,
                    "Couper le son hors focus",
                    &mut self.settings.mute_focus,
                );
            });
            space(ui, 12.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                section(ui, "Entrées & statistiques");
                picker(
                    ui,
                    "Touches système",
                    &mut self.settings.keyboard_capture,
                    &["Toujours", "En plein écran seulement", "Jamais"],
                );
                toggle(ui, "Souris absolue", &mut self.settings.remote_mouse);
                toggle(
                    ui,
                    "Manette en arrière-plan",
                    &mut self.settings.background_gamepad,
                );
                toggle(ui, "Overlay performances Moonlight", &mut self.settings.osd);
            });
            space(ui, 12.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                section(ui, "Appareil");
                ui.horizontal(|ui| {
                    text(ui, "Nom de cet appareil", 14.0, t::SECONDARY);
                    ui.text_edit_singleline(&mut self.settings.device_name);
                });
                ui.horizontal(|ui| {
                    text(ui, "Application à lancer", 14.0, t::SECONDARY);
                    ui.text_edit_singleline(&mut self.settings.app_name);
                });
                ui.horizontal(|ui| {
                    text(ui, "Chemin Moonlight", 14.0, t::SECONDARY);
                    ui.text_edit_singleline(&mut self.settings.engine_path);
                });
                let engine = self.engine();
                text(
                    ui,
                    match engine {
                        Some(ref e) => format!("Moteur détecté : {}", e.program.display()),
                        None => "Moteur Moonlight non détecté".into(),
                    },
                    14.0,
                    if engine.is_some() { t::OK } else { t::ERR },
                );
                text(
                    ui,
                    format!(
                        "PC lié : {}",
                        self.settings
                            .linked_place
                            .as_ref()
                            .map(|p| p.host())
                            .unwrap_or_else(|| "Aucune".into())
                    ),
                    14.0,
                    t::SECONDARY,
                );
                space(ui, 10.0);
                if action(ui, "Oublier ce PC", false) {
                    self.settings.linked_place = None;
                    self.selected = None;
                    self.return_to = Screen::Search;
                    self.screen = Screen::Search;
                }
            });
            space(ui, 12.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                section(ui, "À propos & journaux");
                text(
                    ui,
                    format!("Kinseat Client v{} · GPLv3", env!("CARGO_PKG_VERSION")),
                    14.0,
                    t::SECONDARY,
                );
                space(ui, 10.0);
                if action(ui, "Ouvrir les journaux", false) {
                    match engine::log_dir() {
                        Ok(dir) => {
                            #[cfg(windows)]
                            let result =
                                std::process::Command::new("explorer.exe").arg(dir).spawn();
                            #[cfg(not(windows))]
                            let result = std::process::Command::new("xdg-open").arg(dir).spawn();
                            if let Err(e) = result {
                                self.notice = Some(format!("Ouverture impossible : {e}"));
                            }
                        }
                        Err(e) => self.notice = Some(format!("Journaux indisponibles : {e}")),
                    }
                }
            });
        });
        if self.settings != before {
            self.persist();
        }
    }
}
