use crate::notice::TimedNotice;
use crate::session::Command;
use crate::theme as t;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind};
use kinseat_ipc::{Health, NoticePhase, Seat};
use std::collections::HashMap;

pub enum DeviceIntent {
    Associate,
    Remove(String, String),
}

pub fn devices(
    ui: &mut egui::Ui,
    seats: &[Seat],
    clients: &HashMap<String, Vec<String>>,
    demo: bool,
    error: Option<&str>,
) -> Option<DeviceIntent> {
    let mut intent = None;
    ui.horizontal(|ui| {
        ui.set_min_height(34.0);
        ui.label(t::text(
            "Appareils associés à chaque place",
            15.0,
            t::SECONDARY,
        ));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(t::text("+ Associer un appareil", 14.0, t::ON_ACCENT))
                        .fill(t::ACCENT)
                        .corner_radius(6)
                        .min_size(vec2(170.0, 30.0)),
                )
                .clicked()
            {
                intent = Some(DeviceIntent::Associate);
            }
        });
    });
    if demo {
        ui.label(t::text(
            "Données fictives — aucun appareil réel",
            12.0,
            t::WARN,
        ));
    }
    if let Some(error) = error {
        ui.label(t::text(error, 13.0, t::ERR));
    }
    ui.add_space(16.0);
    if seats.is_empty() {
        ui.label(t::text("Aucune place disponible", 15.0, t::FAINT));
    }
    for seat in seats {
        t::frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(t::title(&seat.account_name, 17.0));
                let (label, color) = status(&seat.status);
                badge(ui, label, color);
            });
            if let Some(names) = clients.get(&seat.id) {
                if names.is_empty() {
                    ui.label(t::text("Aucun appareil associé", 13.0, t::FAINT));
                }
                for name in names {
                    ui.horizontal(|ui| {
                        ui.set_min_height(30.0);
                        ui.label(t::text(name, 14.0, t::PRIMARY));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(egui::Button::new("Retirer").min_size(vec2(76.0, 26.0)))
                                .clicked()
                            {
                                intent = Some(DeviceIntent::Remove(seat.id.clone(), name.clone()));
                            }
                        });
                    });
                }
            } else {
                ui.label(t::text("Chargement des appareils…", 13.0, t::FAINT));
            }
        });
        ui.add_space(10.0);
    }
    intent
}

pub fn status(value: &str) -> (&'static str, Color32) {
    match value {
        "Streaming" => ("Joue", t::OK),
        "Ready" => ("Prêt", t::FAINT),
        "Idle" => ("Inactif", t::FAINT),
        "Provisioning" => ("Préparation", t::WARN),
        "Configuring" => ("Configuration", t::WARN),
        "Connecting" => ("Connexion", t::WARN),
        "TearingDown" => ("Arrêt en cours", t::WARN),
        "Error" => ("Problème", t::ERR),
        "Recovering" => ("Rétablissement", t::WARN),
        _ => ("État inconnu", t::WARN),
    }
}

pub fn health_label(value: &str) -> (&'static str, Color32) {
    match value {
        "Healthy" => ("Bon état", t::OK),
        "Warning" => ("À surveiller", t::WARN),
        _ => ("Problème", t::ERR),
    }
}

pub fn issue(message: &str) -> (String, &'static str) {
    for (prefix, translated, advice) in [
        (
            "CPU critical: ",
            "Processeur saturé : ",
            "Fermez les applications gourmandes avant de lancer une autre place.",
        ),
        (
            "CPU high: ",
            "Processeur très sollicité : ",
            "Surveillez les jeux en cours et fermez les applications inutiles.",
        ),
        (
            "Memory critical: ",
            "Mémoire presque épuisée : ",
            "Fermez des applications pour libérer de la mémoire.",
        ),
        (
            "Memory high: ",
            "Mémoire très utilisée : ",
            "Surveillez la mémoire avant d’ouvrir un autre jeu.",
        ),
        (
            "GPU near capacity: ",
            "Carte graphique proche de sa limite : ",
            "Réduisez la charge graphique ou le nombre de flux.",
        ),
    ] {
        if let Some(value) = message.strip_prefix(prefix) {
            return (format!("{translated}{value}"), advice);
        }
    }
    if message == "RDP Wrapper inactive — multi-session unavailable" {
        return (
            "Une seule session Windows à la fois".into(),
            "Sans l'option Plusieurs sessions, démarrer une place déconnecte l'autre utilisateur. Activez-la dans Paramètres.",
        );
    }
    (
        format!("Signalement du service (non traduit) : {message}"),
        "Consultez le service pour plus de détails.",
    )
}

fn controller(seat: &Seat) -> &'static str {
    if seat.vi_gem_controller_index.is_some_and(|i| i >= 0) {
        "Manette connectée"
    } else {
        "Sans manette"
    }
}

fn activity(seat: &Seat) -> String {
    match seat.status.as_str() {
        "Streaming" => seat
            .launch_app
            .as_deref()
            .map(|app| format!("Programme configuré : {app}"))
            .unwrap_or_else(|| "Sur le bureau".into()),
        "Ready" => "Prêt — en attente de connexion".into(),
        "Error" => "La place rencontre un problème".into(),
        "Provisioning" | "Configuring" | "Connecting" => seat
            .provisioning_step
            .as_deref()
            .map(|s| format!("Démarrage : {s}"))
            .unwrap_or_else(|| "Démarrage de la session…".into()),
        _ => status(&seat.status).0.into(),
    }
}

/// Texte peint sur une seule ligne, tronqué par « … » s'il dépasse `width`.
fn line(p: &egui::Painter, at: egui::Pos2, text: &str, font: FontId, color: Color32, width: f32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(10.0));
    let galley = p.layout_job(job);
    p.galley(at, galley, color);
}

/// Pastille + libellé : le statut n'est jamais porté par la couleur seule.
fn dot_label(p: &egui::Painter, at: egui::Pos2, label: &str, color: Color32) -> f32 {
    p.circle_filled(pos2(at.x + 4.0, at.y + 8.0), 4.0, color);
    let g = p.layout_no_wrap(label.to_owned(), FontId::proportional(12.0), t::PRIMARY);
    let w = g.size().x;
    p.galley(pos2(at.x + 14.0, at.y), g, t::PRIMARY);
    14.0 + w
}

pub fn badge(ui: &mut egui::Ui, label: &str, color: Color32) {
    let g = ui
        .painter()
        .layout_no_wrap(label.to_owned(), FontId::proportional(12.0), t::PRIMARY);
    let (rect, _) = ui.allocate_exact_size(vec2(14.0 + g.size().x, 16.0), Sense::hover());
    dot_label(ui.painter(), rect.min, label, color);
}

/// Avatar dessiné pour Kinseat : silhouette stable par membre, sans prétendre
/// représenter le visage réel de la personne.
pub fn avatar(p: &egui::Painter, center: egui::Pos2, radius: f32, name: &str) {
    let k = radius / 21.0;
    let light = t::PRIMARY;
    p.circle_filled(center, radius, t::member_color(name));
    let shoulders: Vec<egui::Pos2> = (0..=16)
        .map(|i| {
            let a = std::f32::consts::PI * (1.0 + i as f32 / 16.0);
            center + vec2(12.0 * k * a.cos(), (15.0 + 9.0 * a.sin()) * k)
        })
        .collect();
    p.add(egui::Shape::convex_polygon(
        shoulders,
        light.gamma_multiply(0.85),
        Stroke::NONE,
    ));
    p.circle_filled(center + vec2(0.0, -4.0 * k), 7.5 * k, light);
    if radius >= 16.0 {
        p.circle_filled(center + vec2(-2.6 * k, -4.5 * k), 1.1 * k, t::ON_ACCENT);
        p.circle_filled(center + vec2(2.6 * k, -4.5 * k), 1.1 * k, t::ON_ACCENT);
        p.line_segment(
            [
                center + vec2(-1.8 * k, -1.2 * k),
                center + vec2(1.8 * k, -1.2 * k),
            ],
            Stroke::new(1.0_f32, t::ON_ACCENT),
        );
    }
}

fn dashed_rect(p: &egui::Painter, r: Rect, color: Color32) {
    let s = Stroke::new(1.0_f32, color);
    let mut x = r.left() + 10.0;
    while x < r.right() - 10.0 {
        let e = (x + 6.0).min(r.right() - 10.0);
        p.line_segment([pos2(x, r.top()), pos2(e, r.top())], s);
        p.line_segment([pos2(x, r.bottom()), pos2(e, r.bottom())], s);
        x += 12.0;
    }
    let mut y = r.top() + 10.0;
    while y < r.bottom() - 10.0 {
        let e = (y + 6.0).min(r.bottom() - 10.0);
        p.line_segment([pos2(r.left(), y), pos2(r.left(), e)], s);
        p.line_segment([pos2(r.right(), y), pos2(r.right(), e)], s);
        y += 12.0;
    }
}

const TILE_H: f32 = 204.0;

fn seat_tile(
    p: &egui::Painter,
    r: Rect,
    seat: Option<&Seat>,
    place: usize,
    notice: Option<&TimedNotice>,
) {
    let Some(seat) = seat else {
        dashed_rect(p, r.shrink(0.5), t::BORDER);
        p.text(
            pos2(r.left() + 16.0, r.top() + 16.0),
            Align2::LEFT_TOP,
            format!("Place {place}"),
            FontId::proportional(12.0),
            t::FAINT,
        );
        p.text(
            r.center() - vec2(0.0, 8.0),
            Align2::CENTER_CENTER,
            "Place libre",
            t::title_font(17.0),
            t::SECONDARY,
        );
        p.text(
            r.center() + vec2(0.0, 16.0),
            Align2::CENTER_CENTER,
            "Disponible pour un membre",
            FontId::proportional(12.0),
            t::FAINT,
        );
        return;
    };
    p.rect_filled(r, CornerRadius::same(10), t::SURFACE);
    let (label, color) = status(&seat.status);
    if seat.status == "Streaming" || seat.status == "Error" {
        // Liseré d'état en haut de la tuile, en plus de la pastille libellée.
        p.rect_filled(
            Rect::from_min_size(r.min + vec2(16.0, 0.0), vec2(r.width() - 32.0, 2.0)),
            CornerRadius::same(1),
            color,
        );
    }
    let inner = r.width() - 32.0;
    avatar(
        p,
        pos2(r.left() + 34.0, r.top() + 36.0),
        18.0,
        &seat.account_name,
    );
    line(
        p,
        pos2(r.left() + 64.0, r.top() + 16.0),
        &seat.account_name,
        t::title_font(17.0),
        t::PRIMARY,
        inner - 48.0,
    );
    dot_label(p, pos2(r.left() + 64.0, r.top() + 40.0), label, color);
    p.text(
        pos2(r.right() - 16.0, r.top() + 18.0),
        Align2::RIGHT_TOP,
        format!("Place {place}"),
        FontId::proportional(12.0),
        t::FAINT,
    );
    line(
        p,
        pos2(r.left() + 16.0, r.top() + 82.0),
        &activity(seat),
        FontId::proportional(14.0),
        t::PRIMARY,
        inner,
    );
    line(
        p,
        pos2(r.left() + 16.0, r.top() + 118.0),
        &format!("{} × {} · {} images/s", seat.width, seat.height, seat.fps),
        FontId::proportional(12.0),
        t::SECONDARY,
        inner,
    );
    line(
        p,
        pos2(r.left() + 16.0, r.top() + 138.0),
        controller(seat),
        FontId::proportional(12.0),
        t::SECONDARY,
        inner,
    );
    if let Some(notice) = notice {
        // Bande « fin de session » en pied de tuile : le compte à rebours est l'information
        // principale, la phase vient en second ; le bouton Annuler est posé à droite par l'appelant.
        let band = Rect::from_min_max(pos2(r.left(), r.bottom() - 48.0), r.max);
        p.rect_filled(
            band,
            CornerRadius {
                nw: 0,
                ne: 0,
                sw: 10,
                se: 10,
            },
            t::HOVER,
        );
        let clock = crate::notice::clock(notice.remaining());
        let g = p.layout_no_wrap(clock, t::title_font(21.0), notice.color());
        let cw = g.size().x;
        p.galley(
            pos2(band.left() + 16.0, band.center().y - g.size().y / 2.0),
            g,
            notice.color(),
        );
        line(
            p,
            pos2(band.left() + 26.0 + cw, band.center().y - 8.0),
            match notice.notice.phase {
                NoticePhase::Pending => "En attente…",
                NoticePhase::Sending => "Envoi…",
                NoticePhase::Displayed => "Affiché",
                NoticePhase::Closing => "Fermeture…",
                _ => "Fin de session",
            },
            FontId::proportional(12.0),
            t::SECONDARY,
            band.width() - cw - 26.0 - 96.0,
        );
    }
}

struct Meter {
    name: &'static str,
    detail: String,
    value: String,
    ratio: Option<f32>,
    color: Color32,
}

fn meter(p: &egui::Painter, r: Rect, m: &Meter) {
    p.text(
        r.left_top(),
        Align2::LEFT_TOP,
        m.name,
        FontId::proportional(12.0),
        t::SECONDARY,
    );
    p.text(
        r.right_top() - vec2(0.0, 2.0),
        Align2::RIGHT_TOP,
        &m.value,
        t::title_font(14.0),
        t::PRIMARY,
    );
    if let Some(ratio) = m.ratio {
        let track = Rect::from_min_size(pos2(r.left(), r.top() + 22.0), vec2(r.width(), 6.0));
        p.rect_filled(track, CornerRadius::same(3), t::HOVER);
        let mut fill = track;
        fill.set_width(track.width() * ratio.clamp(0.0, 1.0));
        let color = if ratio >= 0.9 { t::WARN } else { m.color };
        p.rect_filled(fill, CornerRadius::same(3), color);
    }
    line(
        p,
        pos2(r.left(), r.top() + 34.0),
        &m.detail,
        FontId::proportional(12.0),
        t::FAINT,
        r.width(),
    );
}

fn meters(health: &Health) -> Vec<Meter> {
    let mut out = Vec::new();
    if let Some(cpu) = &health.cpu {
        out.push(Meter {
            name: "Processeur",
            detail: format!("{} · {} cœurs logiques", cpu.name, cpu.logical_cores),
            value: format!("{} %", cpu.utilization_percent),
            ratio: Some(cpu.utilization_percent as f32 / 100.0),
            color: t::SECONDARY,
        });
    }
    if let Some(gpu) = &health.gpu {
        out.push(Meter {
            name: "Carte graphique",
            detail: format!(
                "{} · {:.1} / {:.1} Go de mémoire vidéo",
                gpu.name,
                gpu.vram_used_mb as f64 / 1024.0,
                gpu.vram_total_mb as f64 / 1024.0
            ),
            value: format!("{} %", gpu.utilization_percent),
            ratio: Some(gpu.utilization_percent as f32 / 100.0),
            color: t::ACCENT,
        });
        out.push(Meter {
            name: "Encodeur vidéo",
            detail: format!("{} flux en cours d’envoi", gpu.active_encoder_sessions),
            value: format!("{} %", gpu.encoder_utilization_percent),
            ratio: Some(gpu.encoder_utilization_percent as f32 / 100.0),
            color: t::ACCENT,
        });
    }
    if health.system_memory_mb > 0 {
        let used = health
            .system_memory_mb
            .saturating_sub(health.available_memory_mb);
        out.push(Meter {
            name: "Mémoire",
            detail: format!(
                "{:.1} Go libres",
                health.available_memory_mb as f64 / 1024.0
            ),
            value: format!(
                "{:.1} / {:.0} Go",
                used as f64 / 1024.0,
                health.system_memory_mb as f64 / 1024.0
            ),
            ratio: Some(used as f32 / health.system_memory_mb as f32),
            color: t::SECONDARY,
        });
    }
    if let Some(disk) = health.disks.first() {
        out.push(Meter {
            name: "Disque système",
            detail: format!(
                "{} · {:.0} Go libres",
                disk.name,
                disk.free_mb as f64 / 1024.0
            ),
            value: format!("{} %", disk.used_percent),
            ratio: Some(disk.used_percent as f32 / 100.0),
            color: t::SECONDARY,
        });
    }
    if let Some(net) = &health.network {
        out.push(Meter {
            name: "Réseau",
            detail: net.primary_adapter.clone(),
            value: format!(
                "↓ {:.1}  ↑ {:.1} Mo/s",
                net.bytes_received_per_sec as f64 / 1_000_000.0,
                net.bytes_sent_per_sec as f64 / 1_000_000.0
            ),
            ratio: None,
            color: t::SECONDARY,
        });
    }
    out
}

fn section(ui: &mut egui::Ui, title: &str, hint: Option<&str>) {
    ui.add_space(32.0);
    ui.horizontal(|ui| {
        ui.label(t::title(title, 17.0));
        if let Some(hint) = hint {
            ui.add_space(8.0);
            ui.label(t::text(hint, 12.0, t::FAINT));
        }
    });
    ui.add_space(12.0);
}

pub fn overview(
    ui: &mut egui::Ui,
    health: &Health,
    seats: &[Seat],
    notices: &[TimedNotice],
) -> Option<Command> {
    let mut command = None;
    let occupied = seats.iter().filter(|s| s.status == "Streaming").count();
    let total = (health.max_seats as usize).max(seats.len());

    // En-tête : le chiffre des places (le geste fort de l'écran) et l'état du PC à droite.
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
    let p = ui.painter();
    let n = p.layout_no_wrap(occupied.to_string(), t::title_font(40.0), t::PRIMARY);
    let nw = n.size().x;
    p.galley(pos2(head.left(), head.top()), n, t::PRIMARY);
    p.text(
        pos2(head.left() + nw + 12.0, head.top() + 8.0),
        Align2::LEFT_TOP,
        format!("sur {total} places en train de jouer"),
        FontId::proportional(17.0),
        t::SECONDARY,
    );
    let ready = seats.iter().filter(|s| s.status == "Ready").count();
    p.text(
        pos2(head.left() + nw + 12.0, head.top() + 30.0),
        Align2::LEFT_TOP,
        {
            let free = total.saturating_sub(seats.len());
            let pl =
                |n: usize, one: &str, many: &str| format!("{n} {}", if n > 1 { many } else { one });
            format!(
                "{} · {}",
                pl(ready, "prête", "prêtes"),
                pl(free, "libre", "libres")
            )
        },
        FontId::proportional(12.0),
        t::FAINT,
    );
    let (hl, hc) = health_label(&health.health.status);
    let pill_text = format!("{hl} · {}/100", health.health.score);
    let g = p.layout_no_wrap(pill_text.clone(), FontId::proportional(14.0), t::PRIMARY);
    let pill = Rect::from_min_size(
        pos2(head.right() - g.size().x - 40.0, head.top() + 4.0),
        vec2(g.size().x + 40.0, 32.0),
    );
    p.rect_filled(pill, CornerRadius::same(16), t::SURFACE);
    p.circle_filled(pos2(pill.left() + 18.0, pill.center().y), 5.0, hc);
    p.galley(
        pos2(pill.left() + 30.0, pill.center().y - g.size().y / 2.0),
        g,
        t::PRIMARY,
    );

    // Les places : une tuile par place, libres comprises, pour voir la capacité.
    section(ui, "Les places", None);
    let width = ui.available_width();
    let cols = if width >= 900.0 { 4 } else { 2 };
    let gap = 12.0;
    let tile_w = (width - gap * (cols as f32 - 1.0)) / cols as f32;
    let rows = total.div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(
        vec2(
            width,
            rows as f32 * TILE_H + (rows.saturating_sub(1)) as f32 * gap,
        ),
        Sense::hover(),
    );
    for i in 0..total {
        let (row, col) = (i / cols, i % cols);
        let r = Rect::from_min_size(
            pos2(
                area.left() + col as f32 * (tile_w + gap),
                area.top() + row as f32 * (TILE_H + gap),
            ),
            vec2(tile_w, TILE_H),
        );
        let notice = seats.get(i).and_then(|s| {
            notices
                .iter()
                .find(|n| n.active() && n.notice.seat_id == s.id)
        });
        seat_tile(ui.painter(), r, seats.get(i), i + 1, notice);
        if let (Some(seat), Some(n)) = (seats.get(i), notice) {
            if n.notice.phase != NoticePhase::Closing {
                let cancel = Rect::from_min_size(
                    pos2(r.right() - 88.0, r.bottom() - 38.0),
                    vec2(76.0, 28.0),
                );
                let button = egui::Button::new(t::text("Annuler", 12.0, t::PRIMARY))
                    .fill(t::SURFACE)
                    .corner_radius(6);
                if ui.put(cancel, button).clicked() {
                    command = Some(Command::Cancel(seat.id.clone()));
                }
            }
        }
    }

    // La machine : six mesures en 3 colonnes.
    section(ui, "La machine", Some(&health.windows_build));
    let list = meters(health);
    let mcols = if width >= 900.0 { 3 } else { 2 };
    let mrows = list.len().div_ceil(mcols);
    let mh = 52.0;
    let (band, _) = ui.allocate_exact_size(
        vec2(
            width,
            32.0 + mrows as f32 * mh + (mrows.saturating_sub(1)) as f32 * 16.0,
        ),
        Sense::hover(),
    );
    ui.painter()
        .rect_filled(band, CornerRadius::same(10), t::SURFACE);
    let mgap = 32.0;
    let mw = (band.width() - 32.0 - mgap * (mcols as f32 - 1.0)) / mcols as f32;
    for (i, m) in list.iter().enumerate() {
        let (row, col) = (i / mcols, i % mcols);
        let r = Rect::from_min_size(
            pos2(
                band.left() + 16.0 + col as f32 * (mw + mgap),
                band.top() + 16.0 + row as f32 * (mh + 16.0),
            ),
            vec2(mw, mh),
        );
        meter(ui.painter(), r, m);
    }

    if !health.health.issues.is_empty() {
        section(ui, "À surveiller", None);
        for message in &health.health.issues {
            let (label, advice) = issue(message);
            let (r, _) = ui.allocate_exact_size(vec2(width, 64.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(r, CornerRadius::same(10), t::SURFACE);
            p.rect_filled(
                Rect::from_min_size(r.min, vec2(3.0, r.height())),
                CornerRadius::same(1),
                t::WARN,
            );
            let w = dot_label(
                p,
                pos2(r.left() + 16.0, r.top() + 14.0),
                "Attention",
                t::WARN,
            );
            line(
                p,
                pos2(r.left() + 32.0 + w, r.top() + 12.0),
                &label,
                FontId::proportional(14.0),
                t::PRIMARY,
                r.width() - w - 48.0,
            );
            line(
                p,
                pos2(r.left() + 16.0, r.top() + 38.0),
                advice,
                FontId::proportional(12.0),
                t::SECONDARY,
                r.width() - 32.0,
            );
            ui.add_space(8.0);
        }
    }
    ui.add_space(24.0);
    command
}

const COLS: [(&str, f32); 5] = [
    ("Membre", 0.0),
    ("État", 0.27),
    ("Activité", 0.42),
    ("Affichage", 0.72),
    ("Session", 0.89),
];

/// Colonnes de la liste des places selon la largeur utile : la résolution et la session ont
/// une largeur fixe (jamais tronquées), la colonne Session disparaît sous 700 px.
/// Retourne les positions relatives et l'indicateur d'affichage de la colonne Session.
fn seat_cols(inner: f32) -> ([f32; 5], bool) {
    let inner = inner.max(1.0);
    let show_session = inner >= 700.0;
    let right = inner - 32.0;
    let session = if show_session { right - 72.0 } else { right };
    let display = session - 150.0;
    let member = (inner * 0.2).clamp(130.0, 230.0).min(display * 0.3);
    let state = member;
    let activity = state + (inner * 0.12).clamp(110.0, 150.0);
    let activity = activity.min(display - 60.0);
    (
        [
            0.0,
            state / inner,
            activity / inner,
            display / inner,
            session / inner,
        ],
        show_session,
    )
}

pub enum SeatIntent {
    Add,
    Delete(String),
    Settings(String),
    Preview(String),
    Schedule(String),
    ConfirmClose(String),
    Command(Command),
    ClearHistory(Vec<String>),
}

const RECENT_NOTICES_SHOWN: usize = 5;

pub fn seats(
    ui: &mut egui::Ui,
    seats: &[Seat],
    notices: &[TimedNotice],
    reduced: bool,
    highlight: Option<&str>,
    actions_pending: &std::collections::HashMap<String, &'static str>,
    hidden_notices: &std::collections::HashSet<String>,
) -> Option<SeatIntent> {
    let mut intent = None;
    let add = |ui: &mut egui::Ui| {
        ui.add(
            egui::Button::new(t::title("Ajouter une place", 14.0).color(t::ON_ACCENT))
                .fill(t::ACCENT)
                .corner_radius(6)
                .min_size(vec2(170.0, 34.0)),
        )
        .clicked()
    };
    if seats.is_empty() && notices.is_empty() {
        empty(ui, "Aucune place pour l’instant", "Une place associe un membre (compte Windows) à un écran de jeu diffusé vers son appareil. Ajoutez-en une pour commencer.");
        ui.add_space(16.0);
        if add(ui) {
            return Some(SeatIntent::Add);
        }
        return None;
    }
    let width = ui.available_width();
    ui.horizontal(|ui| {
        ui.set_min_height(34.0);
        ui.label(t::text(
            "Cliquez sur une place pour voir son détail.",
            12.0,
            t::FAINT,
        ));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if add(ui) {
                intent = Some(SeatIntent::Add);
            }
        });
    });
    ui.add_space(12.0);
    if seats.is_empty() {
        empty(
            ui,
            "Aucune place pour l’instant",
            "Ajoutez une place pour qu’un membre puisse se connecter depuis son appareil.",
        );
    }
    let (head, _) = ui.allocate_exact_size(
        vec2(width, if seats.is_empty() { 0.0 } else { 20.0 }),
        Sense::hover(),
    );
    let (cols, show_session) = seat_cols(width - 32.0);
    for (i, (name, _)) in COLS.iter().enumerate().filter(|_| !seats.is_empty()) {
        if i == 4 && !show_session {
            continue;
        }
        let x = cols[i];
        let name = *name;
        ui.painter().text(
            pos2(head.left() + 16.0 + x * (width - 32.0), head.top()),
            Align2::LEFT_TOP,
            name,
            FontId::proportional(12.0),
            t::FAINT,
        );
    }
    ui.add_space(4.0);
    for seat in seats {
        let id = ui.make_persistent_id(("seat-row", &seat.id));
        let mut open = ui.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
        let notice = notices
            .iter()
            .find(|n| n.active() && n.notice.seat_id == seat.id);
        let (r, resp) = ui.allocate_exact_size(
            vec2(width, if notice.is_some() { 94.0 } else { 56.0 }),
            Sense::click(),
        );
        if resp.clicked() {
            open = !open;
            ui.data_mut(|d| d.insert_temp(id, open));
        }
        let focused = highlight == Some(seat.id.as_str());
        if focused {
            ui.scroll_to_rect(r, Some(egui::Align::Center));
        }
        let r = t::card_hover(
            ui,
            id.with("hover"),
            resp.hovered() || open || focused,
            reduced,
            r,
        );
        if focused {
            ui.painter()
                .rect_stroke(r, 6.0, Stroke::new(2.0_f32, t::ACCENT), StrokeKind::Inside);
        }
        let cy = if notice.is_some() {
            r.top() + 28.0
        } else {
            r.center().y
        };
        let p = ui.painter();
        let col = |i: usize| r.left() + 16.0 + cols[i] * (width - 32.0);
        let cw = |i: usize| {
            let next = if i + 1 < cols.len() { cols[i + 1] } else { 1.0 };
            (next - cols[i]) * (width - 32.0) - 12.0
        };
        avatar(p, pos2(col(0) + 14.0, cy), 14.0, &seat.account_name);
        line(
            p,
            pos2(col(0) + 38.0, cy - 10.0),
            &seat.account_name,
            t::title_font(14.0),
            t::PRIMARY,
            cw(0) - 38.0,
        );
        let (label, color) = status(&seat.status);
        dot_label(
            p,
            pos2(col(1), cy - 8.0),
            label,
            t::pulse(ui.ctx(), &seat.status, reduced, color),
        );
        let activity_text = activity(seat);
        let act_rect = Rect::from_min_size(pos2(col(2), cy - 12.0), vec2(cw(2).max(10.0), 24.0));
        ui.interact(act_rect, id.with("activity"), Sense::hover())
            .on_hover_text(activity_text.clone());
        let p = ui.painter();
        line(
            p,
            pos2(col(2), cy - 10.0),
            &activity_text,
            FontId::proportional(14.0),
            t::PRIMARY,
            cw(2),
        );
        line(
            p,
            pos2(col(3), cy - 9.0),
            &format!("{}×{} · {} i/s", seat.width, seat.height, seat.fps),
            FontId::proportional(12.0),
            t::SECONDARY,
            cw(3),
        );
        let session = if seat.session_id >= 0 {
            format!("n° {}", seat.session_id)
        } else {
            "—".into()
        };
        if show_session {
            line(
                p,
                pos2(col(4), cy - 9.0),
                &session,
                FontId::proportional(12.0),
                t::SECONDARY,
                cw(4) - 16.0,
            );
        }
        // Chevron dessiné (la police n'a pas toujours les flèches).
        let c = pos2(r.right() - 24.0, cy);
        let (a, b) = if open { (-3.0, 3.0) } else { (3.0, -3.0) };
        let s = Stroke::new(1.5_f32, t::SECONDARY);
        p.line_segment([pos2(c.x - 5.0, c.y - a), pos2(c.x, c.y - b)], s);
        p.line_segment([pos2(c.x, c.y - b), pos2(c.x + 5.0, c.y - a)], s);
        if let Some(n) = notice {
            dot_label(
                p,
                pos2(r.left() + 16.0, r.top() + 66.0),
                &n.label(),
                n.color(),
            );
            if n.notice.phase != NoticePhase::Closing {
                let cancel =
                    Rect::from_min_size(pos2(r.right() - 92.0, r.top() + 62.0), vec2(76.0, 24.0));
                let button = egui::Button::new(t::text("Annuler", 12.0, t::PRIMARY))
                    .fill(t::BORDER)
                    .corner_radius(6)
                    .small()
                    .min_size(vec2(76.0, 24.0));
                let clicked = ui
                    .scope(|ui| {
                        ui.spacing_mut().interact_size.y = 24.0;
                        ui.put(cancel, button).clicked()
                    })
                    .inner;
                if clicked {
                    intent = Some(SeatIntent::Command(Command::Cancel(seat.id.clone())));
                }
            }
        }
        if open {
            t::frame()
                .fill(t::HOVER)
                .corner_radius(CornerRadius {
                    nw: 0,
                    ne: 0,
                    sw: 10,
                    se: 10,
                })
                .show(ui, |ui| {
                    ui.set_width(width - 32.0);
                    egui::Grid::new(("seat-detail", &seat.id))
                        .num_columns(2)
                        .spacing(vec2(32.0, 8.0))
                        .show(ui, |ui| {
                            let mut row = |k: &str, v: String| {
                                ui.label(t::text(k, 12.0, t::SECONDARY));
                                ui.label(t::text(v, 14.0, t::PRIMARY));
                                ui.end_row();
                            };
                            row("Manette", controller(seat).into());
                            row(
                                "Ports de streaming",
                                format!("à partir de {}", seat.port_base),
                            );
                            row(
                                "Démarrage automatique",
                                if seat.auto_start { "Oui" } else { "Non" }.into(),
                            );
                            if let Some(scale) = seat.scale_factor {
                                row("Mise à l’échelle", format!("{scale} %"));
                            }
                            row(
                                "Active depuis",
                                seat.ready_at
                                    .as_deref()
                                    .or(seat.created_at.as_deref())
                                    .map(short_time)
                                    .unwrap_or_else(|| "date non disponible".into()),
                            );
                            if let Some(w) = &seat.effective_refresh_rate_warning {
                                row("Fréquence", w.clone());
                            }
                        });
                    if let Some(message) = &seat.error_message {
                        ui.add_space(8.0);
                        ui.label(t::text(
                            format!("Problème signalé : {message}"),
                            14.0,
                            t::ERR,
                        ));
                    }
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if actions_pending.contains_key(&seat.id) {
                            ui.add(egui::Spinner::new());
                            ui.label("En cours…");
                        }
                        let busy = actions_pending.contains_key(&seat.id);
                        if ui.button("Aperçu").clicked() { intent = Some(SeatIntent::Preview(seat.id.clone())); }
                        if ui.add(egui::Button::new(t::title("Programmer la fin…", 14.0).color(t::ON_ACCENT)).fill(t::ACCENT)).clicked() {
                            intent = Some(SeatIntent::Schedule(seat.id.clone()));
                        }
                        if ui.add_enabled(!busy, egui::Button::new("Fermer la session")).clicked() {
                            intent = Some(SeatIntent::ConfirmClose(seat.id.clone()));
                        }
                        ui.menu_button("Plus", |ui| {
                            if ui.add_enabled(!busy, egui::Button::new("Déconnecter le flux")).on_hover_text("La session Windows et les jeux restent ouverts.").clicked() {
                                intent = Some(SeatIntent::Command(Command::Action { seat: seat.id.clone(), action: "disconnect" })); ui.close();
                            }
                            if ui.add_enabled(!busy, egui::Button::new("Relancer le flux")).on_hover_text("Relance le serveur de streaming ; la session Windows reste ouverte.").clicked() {
                                intent = Some(SeatIntent::Command(Command::Action { seat: seat.id.clone(), action: "restart_stream" })); ui.close();
                            }
                            if ui.add_enabled(!busy, egui::Button::new(t::text("Fermer toute la session", 14.0, t::ERR))).clicked() {
                                intent = Some(SeatIntent::ConfirmClose(seat.id.clone())); ui.close();
                            }
                            ui.separator();
                            if ui.button(t::text("Réglages de la place…", 14.0, t::PRIMARY)).clicked() {
                                intent = Some(SeatIntent::Settings(seat.id.clone())); ui.close();
                            }
                            if ui.button(t::text("Supprimer la place…", 14.0, t::ERR)).on_hover_text("Ferme la session et retire la place ; le compte Windows et ses fichiers sont conservés.").clicked() {
                                intent = Some(SeatIntent::Delete(seat.id.clone())); ui.close();
                            }
                        });
                    });
                });
        }
        ui.add_space(8.0);
    }
    let finished: Vec<&TimedNotice> = notices
        .iter()
        .filter(|n| !n.active() && !hidden_notices.contains(&n.notice.id))
        .collect();
    if !finished.is_empty() {
        section(ui, "Fins de session récentes", None);
        for n in finished.iter().take(RECENT_NOTICES_SHOWN) {
            t::frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(t::title(&n.notice.account_name, 14.0));
                    badge(ui, &n.label(), n.color());
                    ui.label(t::text(
                        format!("programmée le {}", short_time(&n.notice.created_at_utc)),
                        12.0,
                        t::FAINT,
                    ));
                    ui.label(t::text(
                        format!("Message : « {} »", n.notice.message),
                        12.0,
                        t::SECONDARY,
                    ));
                });
            });
            ui.add_space(8.0);
        }
        ui.horizontal(|ui| {
            let older = finished.len().saturating_sub(RECENT_NOTICES_SHOWN);
            if older > 0 {
                ui.label(t::text(
                    format!(
                        "{older} {} masquée{}",
                        if older > 1 {
                            "plus anciennes"
                        } else {
                            "plus ancienne"
                        },
                        if older > 1 { "s" } else { "" }
                    ),
                    12.0,
                    t::FAINT,
                ));
            }
            if ui.button("Effacer l’historique").clicked() {
                intent = Some(SeatIntent::ClearHistory(
                    finished.iter().map(|n| n.notice.id.clone()).collect(),
                ));
            }
        });
        ui.add_space(8.0);
    }
    intent
}

/// « 2026-11-13T18:00:00Z » → « 13/11 à 18:00 » (affichage court, sans dépendance de date).
fn short_time(iso: &str) -> String {
    let (date, time) = iso.split_once('T').unwrap_or((iso, ""));
    let mut d = date.split('-');
    match (d.next(), d.next(), d.next(), time.get(0..5)) {
        (Some(_), Some(m), Some(day), Some(hm)) => format!("{day}/{m} à {hm}"),
        _ => iso.to_owned(),
    }
}

pub fn empty(ui: &mut egui::Ui, title: &str, explanation: &str) {
    t::frame().show(ui, |ui| {
        ui.set_width(ui.available_width().min(640.0));
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
            let p = ui.painter();
            p.circle_filled(r.center(), 22.0, t::HOVER);
            p.circle_stroke(r.center(), 9.0, Stroke::new(1.5_f32, t::SECONDARY));
            p.line_segment(
                [r.center() + vec2(-4.0, 0.0), r.center() + vec2(4.0, 0.0)],
                Stroke::new(1.5_f32, t::SECONDARY),
            );
            ui.add_space(14.0);
            ui.vertical(|ui| {
                ui.add_space(2.0);
                ui.label(t::title(title, 17.0));
                ui.add_space(6.0);
                ui.label(t::text(explanation, 14.0, t::SECONDARY));
            });
        });
    });
}

#[allow(dead_code)]
fn outline(p: &egui::Painter, r: Rect) {
    p.rect_stroke(
        r,
        CornerRadius::same(10),
        Stroke::new(1.0_f32, t::BORDER),
        StrokeKind::Inside,
    );
}

pub enum MemberIntent {
    Refresh,
    Create,
    AddSeat(String),
    Delete(String),
}

pub fn member_delete_enabled(checked: bool, pending: bool) -> bool {
    checked && !pending
}

pub fn member_not_deletable(reason: Option<&str>) -> &'static str {
    match reason {
        Some("not_managed") => "Ce compte n'a pas été créé par Kinseat.",
        Some("caller") => "Vous ne pouvez pas supprimer votre propre compte.",
        Some("admin") => "Un administrateur ne peut pas être supprimé.",
        Some("console") => "Le compte de la session console ne peut pas être supprimé.",
        _ => "La suppression de ce compte n'est pas disponible.",
    }
}

/// Membres = comptes Windows locaux gérés par Kinseat (source : service, `accounts.list`).
pub fn members(
    ui: &mut egui::Ui,
    accounts: Option<&[kinseat_ipc::Account]>,
    seats: &[Seat],
    clients: &std::collections::HashMap<String, Vec<String>>,
    error: Option<&str>,
) -> Option<MemberIntent> {
    let mut intent = None;
    ui.horizontal(|ui| {
        if ui
            .add(
                egui::Button::new(t::title("Ajouter un membre", 14.0).color(t::ON_ACCENT))
                    .fill(t::ACCENT)
                    .corner_radius(6)
                    .min_size(vec2(170.0, 34.0)),
            )
            .clicked()
        {
            intent = Some(MemberIntent::Create);
        }
        if ui.button("Actualiser").clicked() {
            intent = Some(MemberIntent::Refresh);
        }
    });
    ui.add_space(8.0);
    ui.label(t::text(
        "Chaque membre a son propre compte Windows : fichiers, sauvegardes et réglages séparés.",
        12.0,
        t::FAINT,
    ));
    ui.add_space(12.0);
    if let Some(error) = error {
        ui.label(t::text(error, 14.0, t::ERR));
        ui.add_space(8.0);
    }
    let Some(accounts) = accounts else {
        ui.label(t::text("Chargement des membres…", 14.0, t::SECONDARY));
        return intent;
    };
    if accounts.is_empty() {
        empty(ui, "Aucun membre pour l’instant", "Ajoutez un membre : Kinseat crée pour lui un compte Windows local standard, sans droits d’administration.");
        return intent;
    }
    let width = ui.available_width();
    for account in accounts {
        let (r, _) = ui.allocate_exact_size(vec2(width, 56.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(r, CornerRadius::same(10), t::SURFACE);
        avatar(
            p,
            pos2(r.left() + 30.0, r.center().y),
            14.0,
            &account.username,
        );
        line(
            p,
            pos2(r.left() + 54.0, r.center().y - 10.0),
            &account.username,
            t::title_font(14.0),
            t::PRIMARY,
            (((width - 32.0) * 0.2).clamp(130.0, 230.0).max(190.0) - 46.0).max(60.0),
        );
        let (label, color) = if account.has_seat {
            ("A une place", t::OK)
        } else {
            ("Sans place", t::SECONDARY)
        };
        let state_x = r.left() + 16.0 + ((width - 32.0) * 0.2).clamp(130.0, 230.0).max(190.0);
        dot_label(p, pos2(state_x, r.center().y - 8.0), label, color);
        // Détail de la place et appareils associés : données déjà en mémoire (aucun appel IPC).
        if let Some(seat) = seats.iter().find(|s| s.account_name == account.username) {
            let (st, st_color) = status(&seat.status);
            let detail_x = state_x + 120.0;
            let detail_w = (r.right() - 226.0 - detail_x).max(10.0);
            let dw = dot_label(p, pos2(detail_x, r.center().y - 17.0), st, st_color);
            line(
                p,
                pos2(detail_x + dw + 8.0, r.center().y - 17.0),
                &format!("{}×{} · {} i/s", seat.width, seat.height, seat.fps),
                FontId::proportional(12.0),
                t::SECONDARY,
                (detail_w - dw - 8.0).max(10.0),
            );
            let devices = clients.get(&seat.id).map(Vec::as_slice).unwrap_or(&[]);
            let text = match devices {
                [] => "Aucun appareil associé".to_owned(),
                [one] => format!("Appareil : {one}"),
                many => format!("{} appareils : {}", many.len(), many.join(", ")),
            };
            line(
                p,
                pos2(detail_x, r.center().y + 1.0),
                &text,
                FontId::proportional(12.0),
                t::FAINT,
                detail_w,
            );
        }
        if !account.deletable {
            ui.interact(
                Rect::from_min_size(pos2(r.left() + 54.0, r.top()), vec2(width * 0.35, 56.0)),
                ui.id().with((&account.username, "reason")),
                Sense::hover(),
            )
            .on_hover_text(member_not_deletable(
                account.not_deletable_reason.as_deref(),
            ));
        }
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(
                    pos2(r.right() - 52.0, r.center().y - 15.0),
                    vec2(40.0, 30.0),
                ))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| {
                ui.menu_button("…", |ui| {
                    if account.deletable && ui.button("Supprimer le membre…").clicked() {
                        intent = Some(MemberIntent::Delete(account.username.clone()));
                        ui.close();
                    }
                    if !account.deletable {
                        ui.label("Suppression indisponible")
                            .on_hover_text(member_not_deletable(
                                account.not_deletable_reason.as_deref(),
                            ));
                    }
                });
            },
        );
        if !account.has_seat {
            let b = Rect::from_min_size(
                pos2(r.right() - 216.0, r.center().y - 15.0),
                vec2(150.0, 30.0),
            );
            if ui
                .put(
                    b,
                    egui::Button::new(t::text("Créer sa place", 13.0, t::PRIMARY))
                        .fill(t::BORDER)
                        .corner_radius(6),
                )
                .clicked()
            {
                intent = Some(MemberIntent::AddSeat(account.username.clone()));
            }
        }
        ui.add_space(8.0);
    }
    intent
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_issues_translated_and_unknown_marked() {
        assert!(issue("CPU critical: 95%").0.contains("Processeur saturé"));
        assert!(issue("RDP Wrapper inactive — multi-session unavailable")
            .0
            .contains("Une seule session Windows"));
        assert!(issue("Surprise").0.contains("non traduit"));
        assert_eq!(status("Recovering").0, "Rétablissement");
        assert_eq!(status("UnknownFuture").0, "État inconnu");
        assert_eq!(health_label("Warning").0, "À surveiller");
    }
    #[test]
    fn deletion_confirmation_requires_explicit_check_and_idle_state() {
        assert!(!super::member_delete_enabled(false, false));
        assert!(!super::member_delete_enabled(true, true));
        assert!(super::member_delete_enabled(true, false));
        assert_eq!(
            super::member_not_deletable(Some("caller")),
            "Vous ne pouvez pas supprimer votre propre compte."
        );
    }
    #[test]
    fn short_time_formats() {
        assert_eq!(short_time("2026-11-13T18:05:00Z"), "13/11 à 18:05");
        assert_eq!(short_time("bizarre"), "bizarre");
    }
}
