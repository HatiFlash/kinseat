use crate::{stream_menu, theme as t, Client, Screen, STREAM_SHORTCUTS};
use egui::{Color32, RichText};
use std::{
    net::{TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
struct Snapshot {
    name: String,
    host: String,
    elapsed: String,
    profile: String,
    latency: String,
    controllers: String,
    state: stream_menu::Panel,
}

fn stat_tile(ui: &mut egui::Ui, width: f32, label: &str, value: &str) {
    egui::Frame::new()
        .fill(t::SURFACE)
        .stroke(egui::Stroke::new(1.0_f32, t::BORDER))
        .corner_radius(8)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width((width - 22.0).max(60.0));
            ui.label(RichText::new(label).size(11.0).color(t::FAINT));
            ui.label(RichText::new(value).size(13.0).color(t::PRIMARY));
        });
}

fn draw(
    ui: &mut egui::Ui,
    snapshot: &Snapshot,
    actions: &std::sync::mpsc::Sender<stream_menu::Action>,
) {
    ui.set_width(336.0);
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        let (mark, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
        t::draw_brand_mark(ui.painter(), mark.min, 0.9);
        ui.label(
            RichText::new("Kinseat · Session")
                .size(16.0)
                .color(t::ACCENT)
                .family(t::title_family()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            egui::Frame::new()
                .fill(t::OK.gamma_multiply(0.15))
                .corner_radius(10)
                .inner_margin(egui::Margin::symmetric(8, 3))
                .show(ui, |ui| {
                    ui.label(RichText::new(&snapshot.latency).size(11.5).color(t::OK));
                });
        });
    });
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(8.0);

    // Grille compacte 2x2 des informations de session.
    let tile_w = 158.0;
    ui.horizontal(|ui| {
        stat_tile(ui, tile_w, "Place", &snapshot.name);
        stat_tile(ui, tile_w, "PC hôte", &snapshot.host);
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        stat_tile(ui, tile_w, "Durée", &snapshot.elapsed);
        stat_tile(ui, tile_w, "Profil", &snapshot.profile);
    });
    if snapshot.controllers != "indisponible" {
        ui.add_space(6.0);
        stat_tile(ui, 324.0, "Manettes", &snapshot.controllers);
    }
    ui.add_space(14.0);

    let button = |ui: &mut egui::Ui, title: &str, action| {
        if ui
            .add_sized([324.0, 32.0], egui::Button::new(title).corner_radius(8))
            .clicked()
        {
            let _ = actions.send(action);
        }
    };
    if ui
        .add_sized(
            [324.0, 36.0],
            egui::Button::new(
                RichText::new("Reprendre (Échap)")
                    .size(14.0)
                    .color(t::ON_ACCENT),
            )
            .fill(t::ACCENT)
            .corner_radius(8),
        )
        .clicked()
    {
        let _ = actions.send(stream_menu::Action::Resume);
    }
    ui.add_space(14.0);
    ui.label(RichText::new("ACTIONS RAPIDES").size(11.0).color(t::FAINT));
    ui.add_space(6.0);
    let quick = |ui: &mut egui::Ui, title: &str, key: u8| {
        if ui
            .add_sized(
                [158.0, 32.0],
                egui::Button::new(RichText::new(title).size(12.5).color(t::PRIMARY))
                    .fill(t::SURFACE)
                    .corner_radius(8),
            )
            .clicked()
        {
            let _ = actions.send(stream_menu::Action::Key(key));
        }
    };
    ui.horizontal(|ui| {
        quick(ui, "Statistiques", b'S');
        quick(ui, "Libérer souris", b'Z');
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        quick(ui, "Plein écran", b'X');
        quick(ui, "Coller texte", b'V');
    });
    ui.add_space(6.0);
    button(ui, "Réduire la fenêtre", stream_menu::Action::Key(b'D'));
    button(ui, "Raccourcis clavier", stream_menu::Action::Shortcuts);
    if snapshot.state.shortcuts {
        ui.add_space(4.0);
        egui::Frame::new()
            .fill(t::SURFACE)
            .stroke(egui::Stroke::new(1.0_f32, t::BORDER))
            .corner_radius(8)
            .inner_margin(10)
            .show(ui, |ui| {
                for (keys, name) in STREAM_SHORTCUTS {
                    ui.label(
                        RichText::new(format!("{keys}  ·  {name}"))
                            .size(11.5)
                            .color(t::SECONDARY),
                    );
                }
            });
    }
    ui.add_space(16.0);
    ui.separator();
    ui.add_space(10.0);
    if snapshot.state.confirm_quit {
        ui.colored_label(t::ERR, "Quitter ce flux ?");
        ui.add_space(6.0);
        button(ui, "Confirmer la sortie", stream_menu::Action::Quit);
        button(ui, "Annuler", stream_menu::Action::CancelQuit);
    } else if ui
        .add_sized(
            [324.0, 34.0],
            egui::Button::new(RichText::new("Quitter le flux").color(t::PRIMARY))
                .fill(Color32::from_rgb(0x6b, 0x26, 0x24))
                .corner_radius(8),
        )
        .clicked()
    {
        let _ = actions.send(stream_menu::Action::ConfirmQuit);
    }
}

impl Client {
    pub(crate) fn start_probe(&mut self) {
        let Some(place) = self.selected.as_ref() else {
            return;
        };
        let addr = place.host();
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                let sample = addr.to_socket_addrs().ok().and_then(|mut addresses| {
                    addresses.find_map(|address| {
                        let started = Instant::now();
                        TcpStream::connect_timeout(&address, Duration::from_millis(800))
                            .ok()
                            .map(|_| started.elapsed().as_millis() as u32)
                    })
                });
                if tx.send(sample).is_err() {
                    break;
                }
                for _ in 0..20 {
                    if flag.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        });
        self.probe_stop = Some(stop);
        self.latency_rx = Some(rx);
        self.latency_samples.clear();
    }
    pub(crate) fn stop_panel(&mut self) {
        self.panel.close();
        self.moonlight_pid = None;
        self.stream_started = None;
        if let Some(flag) = self.probe_stop.take() {
            flag.store(true, Ordering::Relaxed);
        }
        self.latency_rx = None;
        self.latency_samples.clear();
        #[cfg(windows)]
        {
            self.hook = None;
            self.shortcut_rx = None;
        }
    }
    fn snapshot(&self) -> Snapshot {
        let (name, host) = if self.demo {
            ("Place démo".into(), "192.0.2.10:48100 (démo)".into())
        } else {
            self.selected
                .as_ref()
                .map(|p| (p.name.clone(), p.host()))
                .unwrap_or_else(|| ("indisponible".into(), "indisponible".into()))
        };
        Snapshot {
            name,
            host,
            elapsed: if self.demo {
                format!(
                    "{} (démo)",
                    stream_menu::duration(
                        self.stream_started
                            .map_or(Duration::ZERO, |start| start.elapsed())
                    )
                )
            } else {
                self.stream_started
                    .map(|start| stream_menu::duration(start.elapsed()))
                    .unwrap_or_else(|| "indisponible".into())
            },
            profile: if self.demo {
                "1920×1080 · 60 i/s · 40 Mb/s · HEVC · Fenêtré sans bordure (démo)".into()
            } else {
                stream_menu::profile(&self.settings)
            },
            latency: if self.demo {
                "18 ms (démo)".into()
            } else {
                stream_menu::latency(&self.latency_samples.iter().copied().collect::<Vec<_>>())
            },
            controllers: if self.demo {
                "1 manette (démo)".into()
            } else {
                "indisponible".into()
            },
            state: self.panel.clone(),
        }
    }
    pub(crate) fn update_panel(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        if let Some(pid) = self.moonlight_pid {
            let window = crate::stream_windows::moonlight_window(pid);
            if !window.is_null()
                && self.settings.stream_menu_shortcut != "Désactivé"
                && self.hook.is_none()
            {
                let (tx, rx) = std::sync::mpsc::channel();
                self.shortcut_rx = Some(rx);
                self.hook = crate::stream_windows::ShortcutHook::start(
                    pid,
                    &self.settings.stream_menu_shortcut,
                    tx,
                    ctx.clone(),
                );
            }
        }
        #[cfg(windows)]
        let toggles = self
            .shortcut_rx
            .as_ref()
            .map_or(0, |rx| rx.try_iter().count());
        #[cfg(windows)]
        for _ in 0..toggles {
            if self.panel.open {
                self.close_panel();
            } else {
                self.panel.toggle();
                #[cfg(windows)]
                crate::engine::diagnostic("panneau ouvert");
            }
        }
        if let Some(rx) = &self.latency_rx {
            while let Ok(sample) = rx.try_recv() {
                if let Some(ms) = sample {
                    self.latency_samples.push_back(ms);
                    if self.latency_samples.len() > 5 {
                        self.latency_samples.pop_front();
                    }
                } else {
                    self.latency_samples.clear();
                }
            }
        }
        while let Ok(action) = self.panel_rx.try_recv() {
            match action {
                stream_menu::Action::Resume => self.close_panel(),
                stream_menu::Action::Shortcuts => self.panel.shortcuts = !self.panel.shortcuts,
                stream_menu::Action::ConfirmQuit => self.panel.confirm_quit = true,
                stream_menu::Action::CancelQuit => self.panel.confirm_quit = false,
                stream_menu::Action::Key(key) => {
                    // D'abord fermer le panneau (retire le crochet et rend le focus à Moonlight),
                    // ensuite seulement injecter le raccourci.
                    self.close_panel();
                    #[cfg(windows)]
                    if let Some(pid) = self.moonlight_pid {
                        // Exécuter hors du thread UI pour éviter de geler l'interface pendant
                        // la remise du focus et l'injection du raccourci (300 ms à 1,3 s).
                        std::thread::spawn(move || {
                            if key == b'Z' {
                                let _ = crate::stream_windows::release_mouse(pid);
                            } else {
                                let _ = crate::stream_windows::send_shortcut(pid, key as u16);
                            }
                        });
                    }
                    #[cfg(not(windows))]
                    let _ = key;
                }
                stream_menu::Action::Quit => {
                    #[cfg(windows)]
                    if let Some(pid) = self.moonlight_pid {
                        let _ = crate::stream_windows::send_shortcut(pid, b'Q' as u16);
                    }
                    if let Some(tx) = self.stop_tx.take() {
                        let _ = tx.send(());
                    }
                    self.close_panel();
                    self.screen = Screen::Home;
                }
            }
        }
        if !self.panel.open {
            return;
        }
        let snapshot = self.snapshot();
        let tx = self.panel_tx.clone();
        if self.demo {
            egui::SidePanel::right("stream-menu-demo")
                .exact_width(360.0)
                .frame(egui::Frame::new().fill(t::RAIL).inner_margin(12.0))
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| draw(ui, &snapshot, &tx));
                });
            return;
        }
        #[cfg(windows)]
        if let Some(pid) = self.moonlight_pid {
            let moonlight = crate::stream_windows::moonlight_window(pid);
            if let Some((_left, top, right, bottom)) =
                crate::stream_windows::monitor_rect(moonlight)
            {
                let scale = ctx.pixels_per_point();
                let width = 360.0;
                let height = (bottom - top) as f32 / scale;
                let builder = egui::ViewportBuilder::default()
                    .with_title("Kinseat — flux")
                    .with_decorations(false)
                    .with_always_on_top()
                    .with_active(false)
                    .with_resizable(false)
                    .with_inner_size([width, height])
                    .with_position([right as f32 / scale - width, top as f32 / scale]);
                let id = egui::ViewportId::from_hash_of("kinseat-stream-panel");
                // eframe 0.32 ne crée un viewport différé qu'après le paint racine,
                // puis sa fenêtre au prochain événement ; l'immédiat la crée ici,
                // même si la fenêtre racine est entièrement derrière Moonlight.
                ctx.show_viewport_immediate(id, builder, move |child, _| {
                    if child.input(|i| i.key_pressed(egui::Key::Escape)) {
                        let _ = tx.send(stream_menu::Action::Resume);
                    }
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().fill(t::RAIL).inner_margin(12.0))
                        .show(child, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| draw(ui, &snapshot, &tx));
                        });
                });
                if let Some(hook) = &self.hook {
                    if !hook.set_panel(crate::stream_windows::panel_window()) {
                        ctx.request_repaint_after(Duration::from_millis(50));
                    }
                }
            } else {
                self.close_panel();
                self.notice = Some("Fenêtre Moonlight introuvable".into());
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (snapshot, tx);
        }
        let _ = ctx;
    }
    fn close_panel(&mut self) {
        self.panel.close();
        #[cfg(windows)]
        if let Some(pid) = self.moonlight_pid {
            if let Some(hook) = &self.hook {
                hook.set_panel(std::ptr::null_mut());
            }
            if !crate::stream_windows::focus_moonlight(pid) {
                self.notice = Some("Impossible de rendre le focus à Moonlight".into());
            }
        }
    }
}
