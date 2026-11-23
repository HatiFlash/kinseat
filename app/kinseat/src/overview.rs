//! Vue d'ensemble « Le salon » : cartes de places épurées, télémétrie lisible, ressources unifiées.
use crate::{notice::TimedNotice, theme as t, views};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind};
use kinseat_ipc::{Health, NoticePhase, Seat};

pub enum Intent {
    Preview(String),
    ConfirmClose(String),
    Seats,
    AddSeat,
    Cancel(String),
}

fn label(p: &egui::Painter, x: f32, y: f32, value: &str, size: f32, color: Color32, width: f32) {
    let mut job = egui::text::LayoutJob::simple_singleline(
        value.to_owned(),
        FontId::proportional(size),
        color,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(12.0));
    p.galley(pos2(x, y), p.layout_job(job), color);
}

fn title_label(
    p: &egui::Painter,
    x: f32,
    y: f32,
    value: &str,
    size: f32,
    color: Color32,
    width: f32,
) {
    let mut job =
        egui::text::LayoutJob::simple_singleline(value.to_owned(), t::title_font(size), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(12.0));
    p.galley(pos2(x, y), p.layout_job(job), color);
}

fn dashed(p: &egui::Painter, r: Rect, color: Color32) {
    let stroke = Stroke::new(1.0_f32, color);
    let mut x = r.left();
    while x < r.right() {
        let end = (x + 6.0).min(r.right());
        p.line_segment([pos2(x, r.top()), pos2(end, r.top())], stroke);
        p.line_segment([pos2(x, r.bottom()), pos2(end, r.bottom())], stroke);
        x += 12.0;
    }
    let mut y = r.top();
    while y < r.bottom() {
        let end = (y + 6.0).min(r.bottom());
        p.line_segment([pos2(r.left(), y), pos2(r.left(), end)], stroke);
        p.line_segment([pos2(r.right(), y), pos2(r.right(), end)], stroke);
        y += 12.0;
    }
}

fn pill(
    p: &egui::Painter,
    right_top: egui::Pos2,
    text: &str,
    fg: Color32,
    bg: Color32,
    dot: Option<Color32>,
) {
    let galley = p.layout_no_wrap(text.to_owned(), FontId::proportional(11.5), fg);
    let pad_x = 8.0;
    let dot_space = if dot.is_some() { 11.0 } else { 0.0 };
    let w = galley.size().x + pad_x * 2.0 + dot_space;
    let h = 22.0;
    let rect = Rect::from_min_size(pos2(right_top.x - w, right_top.y), vec2(w, h));
    p.rect_filled(rect, CornerRadius::same(11), bg);
    if let Some(d) = dot {
        p.circle_filled(pos2(rect.left() + pad_x + 3.0, rect.center().y), 3.2, d);
    }
    p.galley(
        pos2(
            rect.left() + pad_x + dot_space,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        fg,
    );
}

fn card(
    ui: &mut egui::Ui,
    r: Rect,
    seat: Option<&Seat>,
    number: usize,
    notice: Option<&TimedNotice>,
    reduced: bool,
) -> Option<Intent> {
    let Some(seat) = seat else {
        let id = ui.make_persistent_id(("overview-empty", number));
        let response = ui.interact(r, id, Sense::click());
        let hovered = response.hovered();
        let p = ui.painter();
        p.rect_filled(
            r,
            CornerRadius::same(12),
            if hovered { t::SURFACE } else { t::CANVAS },
        );
        dashed(p, r.shrink(1.0), if hovered { t::ACCENT } else { t::BORDER });
        let circle_center = r.center() - vec2(0.0, 22.0);
        p.circle_filled(
            circle_center,
            18.0,
            if hovered {
                t::ACCENT.gamma_multiply(0.22)
            } else {
                t::HOVER
            },
        );
        p.text(
            circle_center,
            Align2::CENTER_CENTER,
            "+",
            t::title_font(20.0),
            if hovered { t::ACCENT } else { t::SECONDARY },
        );
        p.text(
            r.center() + vec2(0.0, 12.0),
            Align2::CENTER_CENTER,
            format!("Place {number} libre"),
            t::title_font(15.0),
            if hovered { t::ACCENT } else { t::PRIMARY },
        );
        p.text(
            r.center() + vec2(0.0, 32.0),
            Align2::CENTER_CENTER,
            "Cliquer pour ajouter un joueur",
            FontId::proportional(11.5),
            t::FAINT,
        );
        if response.clicked() {
            return Some(Intent::AddSeat);
        }
        return None;
    };

    let id = ui.make_persistent_id(("overview-card", &seat.id));
    let hovered = ui.interact(r, id, Sense::hover()).hovered();
    let r = t::card_hover(ui, id.with("hover"), hovered, reduced, r);
    let p = ui.painter();
    if notice.is_some() {
        p.rect_stroke(
            r,
            CornerRadius::same(10),
            Stroke::new(1.0_f32, t::ACCENT.gamma_multiply(0.55)),
            StrokeKind::Inside,
        );
    }

    let inner = r.shrink2(vec2(14.0, 0.0));
    let compact = r.height() < 185.0;

    // En-tête de carte : PLACE N + pastille d'état ou préavis non tronqué.
    label(
        p,
        inner.left(),
        r.top() + 14.0,
        &format!("PLACE {number}"),
        11.0,
        t::FAINT,
        80.0,
    );
    if let Some(n) = notice {
        pill(
            p,
            pos2(inner.right(), r.top() + 10.0),
            &n.label(),
            n.color(),
            n.color().gamma_multiply(0.16),
            Some(n.color()),
        );
    } else {
        let (state, color) = views::status(&seat.status);
        let dot = t::pulse(ui.ctx(), &seat.status, reduced, color);
        pill(
            p,
            pos2(inner.right(), r.top() + 10.0),
            state,
            color,
            color.gamma_multiply(0.15),
            Some(dot),
        );
    }

    // Identité du joueur : avatar + nom + activité.
    let avatar_y = r.top() + if compact { 60.0 } else { 68.0 };
    views::avatar(p, pos2(inner.left() + 20.0, avatar_y), 20.0, &seat.account_name);
    let display_name = seat
        .display_name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&seat.account_name);
    title_label(
        p,
        inner.left() + 50.0,
        avatar_y - 18.0,
        display_name,
        15.5,
        t::PRIMARY,
        (inner.width() - 52.0).max(20.0),
    );
    let activity = if seat.status == "Streaming" {
        seat.launch_app
            .as_deref()
            .unwrap_or("Sur le bureau")
            .to_owned()
    } else if seat.status == "Ready" {
        "En attente de connexion".into()
    } else {
        "Voir le détail de la place".into()
    };
    label(
        p,
        inner.left() + 50.0,
        avatar_y + 3.0,
        &activity,
        12.0,
        t::SECONDARY,
        (inner.width() - 52.0).max(20.0),
    );

    // Chip télémétrie compacte (résolution · ips).
    if !compact {
        let chip_text = format!("{} × {} · {} i/s", seat.width, seat.height, seat.fps);
        let chip_rect = Rect::from_min_size(pos2(inner.left(), r.top() + 106.0), vec2(inner.width(), 24.0));
        p.rect_filled(chip_rect, CornerRadius::same(6), t::CANVAS);
        p.rect_stroke(
            chip_rect,
            CornerRadius::same(6),
            Stroke::new(1.0_f32, t::BORDER),
            StrokeKind::Inside,
        );
        label(
            p,
            chip_rect.left() + 8.0,
            chip_rect.top() + 4.5,
            &chip_text,
            11.5,
            t::SECONDARY,
            chip_rect.width() - 16.0,
        );
    }

    // Ligne de séparation au-dessus des actions.
    p.line_segment(
        [
            pos2(inner.left(), r.bottom() - 48.0),
            pos2(inner.right(), r.bottom() - 48.0),
        ],
        Stroke::new(1.0_f32, t::BORDER),
    );

    // Barre d'actions : deux boutons équilibrés.
    let btn_gap = 8.0;
    let half_w = ((inner.width() - btn_gap) / 2.0).max(32.0);
    let btn_y = r.bottom() - 39.0;
    let left_rect = Rect::from_min_size(pos2(inner.left(), btn_y), vec2(half_w, 28.0));
    let right_rect = Rect::from_min_size(pos2(inner.left() + half_w + btn_gap, btn_y), vec2(half_w, 28.0));

    if let Some(n) = notice {
        if ui
            .put(
                left_rect,
                egui::Button::new(t::text("Aperçu", 12.5, t::ACCENT))
                    .fill(t::HOVER)
                    .corner_radius(6),
            )
            .clicked()
        {
            return Some(Intent::Preview(seat.id.clone()));
        }
        if n.notice.phase != NoticePhase::Closing
            && ui
                .put(
                    right_rect,
                    egui::Button::new(t::text("Annuler", 12.5, t::PRIMARY))
                        .fill(t::HOVER)
                        .corner_radius(6),
                )
                .clicked()
        {
            return Some(Intent::Cancel(seat.id.clone()));
        }
    } else {
        if ui
            .put(
                left_rect,
                egui::Button::new(t::text("Aperçu", 12.5, t::PRIMARY))
                    .fill(t::HOVER)
                    .corner_radius(6),
            )
            .on_hover_text("Voir l’aperçu de la session")
            .clicked()
        {
            return Some(Intent::Preview(seat.id.clone()));
        }
        if ui
            .put(
                right_rect,
                egui::Button::new(t::text("Fermer", 12.5, t::SECONDARY))
                    .fill(t::HOVER)
                    .corner_radius(6),
            )
            .on_hover_text("Fermer la session de cette place")
            .clicked()
        {
            return Some(Intent::ConfirmClose(seat.id.clone()));
        }
    }
    None
}

struct Meter {
    name: String,
    value: String,
    ratio: f32,
    color: Color32,
}

fn go(used_mb: u64, total_mb: u64) -> String {
    let _ = total_mb;
    format!("{:.1} Go", used_mb as f64 / 1024.0).replace('.', ",")
}

fn meters(health: &Health) -> Vec<Meter> {
    let mut list = Vec::new();
    if let Some(cpu) = &health.cpu {
        list.push(Meter {
            name: "Processeur".into(),
            value: format!("{} %", cpu.utilization_percent),
            ratio: cpu.utilization_percent as f32 / 100.0,
            color: if cpu.utilization_percent >= 80 {
                t::WARN
            } else {
                t::ACCENT
            },
        });
    }
    if let Some(gpu) = &health.gpu {
        list.push(Meter {
            name: "Graphique".into(),
            value: format!("{} %", gpu.utilization_percent),
            ratio: gpu.utilization_percent as f32 / 100.0,
            color: t::ACCENT,
        });
        if gpu.vram_total_mb > 0 {
            list.push(Meter {
                name: format!("VRAM · {:.0} Go", gpu.vram_total_mb as f64 / 1024.0),
                value: go(gpu.vram_used_mb, gpu.vram_total_mb),
                ratio: gpu.vram_used_mb as f32 / gpu.vram_total_mb as f32,
                color: t::OK,
            });
        }
        list.push(Meter {
            name: "Encodeur".into(),
            value: format!("{} %", gpu.encoder_utilization_percent),
            ratio: gpu.encoder_utilization_percent as f32 / 100.0,
            color: t::MEMBERS[0],
        });
    }
    if health.system_memory_mb > 0 {
        let used = health
            .system_memory_mb
            .saturating_sub(health.available_memory_mb);
        list.push(Meter {
            name: format!(
                "Mémoire · {:.0} Go",
                health.system_memory_mb as f64 / 1024.0
            ),
            value: go(used, health.system_memory_mb),
            ratio: used as f32 / health.system_memory_mb as f32,
            color: t::MEMBERS[2],
        });
    }
    list
}

fn meter_card(p: &egui::Painter, r: Rect, m: &Meter) {
    p.rect_filled(r, CornerRadius::same(8), t::CANVAS);
    p.rect_stroke(
        r,
        CornerRadius::same(8),
        Stroke::new(1.0_f32, t::BORDER),
        StrokeKind::Inside,
    );
    let inner = r.shrink2(vec2(10.0, 10.0));
    label(p, inner.left(), inner.top(), &m.name, 11.5, t::SECONDARY, inner.width() * 0.65);
    let accent = if m.ratio >= 0.9 { t::WARN } else { m.color };
    p.text(
        pos2(inner.right(), inner.top() + 7.0),
        Align2::RIGHT_CENTER,
        &m.value,
        t::title_font(12.5),
        if m.ratio >= 0.85 { t::WARN } else { t::PRIMARY },
    );
    let bar = Rect::from_min_size(pos2(inner.left(), inner.bottom() - 8.0), vec2(inner.width(), 6.0));
    p.rect_filled(bar, CornerRadius::same(3), t::HOVER);
    let filled_w = (bar.width() * m.ratio.clamp(0.0, 1.0)).max(if m.ratio > 0.0 { 4.0 } else { 0.0 });
    if filled_w > 0.0 {
        p.rect_filled(
            Rect::from_min_size(bar.min, vec2(filled_w, bar.height())),
            CornerRadius::same(3),
            accent,
        );
    }
}

pub fn show(
    ui: &mut egui::Ui,
    health: &Health,
    seats: &[Seat],
    notices: &[TimedNotice],
    reduced: bool,
) -> Option<Intent> {
    let mut intent = None;
    let width = ui.available_width().max(240.0);
    let total = (health.max_seats as usize).max(seats.len());
    let playing = seats.iter().filter(|s| s.status == "Streaming").count();

    ui.horizontal_wrapped(|ui| {
        ui.label(t::title(format!("{playing} / {total} places en jeu"), 26.0));
        if width > 520.0 {
            ui.add_space(12.0);
            let (state, color) = views::health_label(&health.health.status);
            views::badge(ui, state, color);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if seats.len() < total
                && ui
                    .add(
                        egui::Button::new(t::text("+ Nouvelle place", 13.0, t::ON_ACCENT))
                            .fill(t::ACCENT)
                            .corner_radius(8),
                    )
                    .clicked()
            {
                intent = Some(Intent::AddSeat);
            }
            if ui
                .add(
                    egui::Button::new(t::text("Toutes les places", 13.0, t::PRIMARY))
                        .fill(t::HOVER)
                        .corner_radius(8),
                )
                .clicked()
            {
                intent = Some(Intent::Seats);
            }
        });
    });

    ui.label(t::text(
        format!(
            "{} en attente · {}",
            seats.iter().filter(|s| s.status == "Ready").count(),
            match total.saturating_sub(seats.len()) {
                0 => "aucune place libre".to_owned(),
                1 => "1 place libre".to_owned(),
                n => format!("{n} places libres"),
            }
        ),
        13.0,
        t::SECONDARY,
    ));
    ui.add_space(16.0);

    if total == 0 {
        views::empty(
            ui,
            "Aucune place configurée",
            "Aucune place n’est disponible. Vérifiez la configuration du service ; les places apparaîtront ici dès leur création.",
        );
    } else {
        let cols = if width >= 780.0 {
            4
        } else if width >= 440.0 {
            2
        } else {
            1
        };
        let gap = 12.0;
        let w = ((width - gap * (cols - 1) as f32) / cols as f32).max(140.0);
        let height = if width < 520.0 { 166.0 } else { 196.0 };
        let rows = total.div_ceil(cols);
        for row in 0..rows {
            let (area, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
            for col in 0..cols {
                let i = row * cols + col;
                if i >= total {
                    break;
                }
                let rect = Rect::from_min_size(
                    pos2(area.left() + col as f32 * (w + gap), area.top()),
                    vec2(w, height),
                );
                let notice = seats.get(i).and_then(|s| {
                    notices
                        .iter()
                        .find(|n| n.active() && n.notice.seat_id == s.id)
                });
                if let Some(action) = card(ui, rect, seats.get(i), i + 1, notice, reduced) {
                    intent = Some(action);
                }
            }
            if row + 1 < rows {
                ui.add_space(gap);
            }
        }
    }

    ui.add_space(18.0);

    // Carte unifiée : Ressources du PC + bandeau d'alerte intégré.
    let list = meters(health);
    let issues = &health.health.issues;
    t::frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(t::title("Ressources du PC hôte", 16.0));
        ui.add_space(10.0);
        if list.is_empty() {
            ui.label(t::text(
                "Mesures indisponibles sur ce PC",
                13.0,
                t::SECONDARY,
            ));
        } else {
            let avail = ui.available_width().max(200.0);
            let cols = if avail >= 660.0 {
                list.len().min(5)
            } else if avail >= 380.0 {
                3
            } else {
                2
            };
            let gap = 10.0;
            let card_w = ((avail - gap * (cols - 1) as f32) / cols as f32).max(110.0);
            let card_h = 56.0;
            let rows = list.len().div_ceil(cols);
            for row in 0..rows {
                let (area, _) = ui.allocate_exact_size(vec2(avail, card_h), Sense::hover());
                let p = ui.painter();
                for col in 0..cols {
                    let idx = row * cols + col;
                    let Some(m) = list.get(idx) else { break };
                    let rect = Rect::from_min_size(
                        pos2(area.left() + col as f32 * (card_w + gap), area.top()),
                        vec2(card_w, card_h),
                    );
                    meter_card(p, rect, m);
                }
                if row + 1 < rows {
                    ui.add_space(gap);
                }
            }
        }
        if !issues.is_empty() {
            ui.add_space(12.0);
            egui::Frame::new()
                .fill(t::WARN.gamma_multiply(0.11))
                .stroke(Stroke::new(1.0_f32, t::WARN.gamma_multiply(0.32)))
                .corner_radius(8)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for (i, message) in issues.iter().enumerate() {
                        if i > 0 {
                            ui.add_space(6.0);
                        }
                        let (title, advice) = views::issue(message);
                        ui.label(t::text(title, 13.0, t::WARN));
                        ui.label(t::text(advice, 12.0, t::SECONDARY));
                    }
                });
        }
    });

    intent
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn demo_meters_only_real_fields() {
        let (health, _) = crate::demo::data();
        assert_eq!(meters(&health).len(), 5);
        assert!(meters(&health)
            .iter()
            .all(|m| (0.0..=1.0).contains(&m.ratio)));
    }
}
