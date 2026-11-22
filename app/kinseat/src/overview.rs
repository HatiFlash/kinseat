//! Vue d'ensemble « Le salon » : les places d'abord, les mesures vérifiées ensuite.
use crate::{notice::TimedNotice, theme as t, views};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke};
use kinseat_ipc::{Health, NoticePhase, Seat};

pub enum Intent {
    Preview(String),
    ConfirmClose(String),
    Seats,
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

fn dashed(p: &egui::Painter, r: Rect) {
    let stroke = Stroke::new(1.0_f32, t::FAINT);
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

fn card(
    ui: &mut egui::Ui,
    r: Rect,
    seat: Option<&Seat>,
    number: usize,
    notice: Option<&TimedNotice>,
    reduced: bool,
) -> Option<Intent> {
    let p = ui.painter();
    let Some(seat) = seat else {
        dashed(p, r.shrink(1.0));
        p.text(
            r.center() - vec2(0.0, 14.0),
            Align2::CENTER_CENTER,
            format!("Place {number} libre"),
            t::title_font(17.0),
            t::PRIMARY,
        );
        p.text(
            r.center() + vec2(0.0, 14.0),
            Align2::CENTER_CENTER,
            "Aucune session sur cette place",
            FontId::proportional(12.0),
            t::SECONDARY,
        );
        return None;
    };
    let id = ui.make_persistent_id(("overview-card", &seat.id));
    let hovered = ui.interact(r, id, Sense::hover()).hovered();
    let r = t::card_hover(ui, id.with("hover"), hovered, reduced, r);
    let p = ui.painter();
    let inner = r.shrink2(vec2(12.0, 0.0));
    let compact = r.height() < 200.0;
    let preview_height = if compact { 66.0 } else { 88.0 };
    let preview = Rect::from_min_max(
        pos2(r.left() + 1.0, r.top() + 1.0),
        pos2(r.right() - 1.0, r.top() + preview_height),
    );
    p.rect_filled(preview, CornerRadius::same(8), t::HOVER);
    label(
        p,
        inner.left(),
        r.top() + 10.0,
        &format!("PLACE {number}"),
        12.0,
        t::SECONDARY,
        inner.width(),
    );
    label(
        p,
        inner.left(),
        r.top() + 43.0,
        "Aperçu sur demande",
        14.0,
        t::PRIMARY,
        inner.width(),
    );
    views::avatar(
        p,
        pos2(
            r.left() + 33.0,
            r.top() + if compact { 94.0 } else { 120.0 },
        ),
        21.0,
        &seat.account_name,
    );
    label(
        p,
        r.left() + 64.0,
        r.top() + if compact { 72.0 } else { 99.0 },
        &seat.account_name,
        17.0,
        t::PRIMARY,
        (r.width() - 76.0).max(12.0),
    );
    let (state, color) = views::status(&seat.status);
    p.circle_filled(
        pos2(
            r.left() + 68.0,
            r.top() + if compact { 103.0 } else { 130.0 },
        ),
        4.0,
        t::pulse(ui.ctx(), &seat.status, reduced, color),
    );
    label(
        p,
        r.left() + 79.0,
        r.top() + if compact { 94.0 } else { 121.0 },
        state,
        13.0,
        t::SECONDARY,
        r.width() - 90.0,
    );
    let activity = if seat.status == "Streaming" {
        if let Some(app) = &seat.launch_app {
            format!("Programme configuré : {app}")
        } else {
            "Sur le bureau".into()
        }
    } else if seat.status == "Ready" {
        "En attente de connexion".into()
    } else {
        "Consulter le détail de la place".into()
    };
    if !compact {
        label(
            p,
            inner.left(),
            r.top() + 155.0,
            &activity,
            12.0,
            t::SECONDARY,
            inner.width(),
        );
        label(
            p,
            inner.left(),
            r.top() + 177.0,
            &format!("{} × {} · {} i/s", seat.width, seat.height, seat.fps),
            12.0,
            t::FAINT,
            inner.width(),
        );
    }
    if let Some(n) = notice {
        let message = format!("Préavis {}", n.label());
        label(
            p,
            inner.left(),
            r.bottom() - 33.0,
            &message,
            12.0,
            n.color(),
            inner.width() - 100.0,
        );
        if n.notice.phase != NoticePhase::Closing {
            let button =
                Rect::from_min_size(pos2(r.right() - 92.0, r.bottom() - 40.0), vec2(80.0, 28.0));
            if ui
                .put(button, egui::Button::new("Annuler").fill(t::HOVER))
                .clicked()
            {
                return Some(Intent::Cancel(seat.id.clone()));
            }
        }
    } else {
        if ui
            .put(
                Rect::from_min_size(
                    pos2(inner.left(), r.bottom() - 40.0),
                    vec2(80.0_f32.min(inner.width() * 0.4), 28.0),
                ),
                egui::Button::new("Aperçu")
                    .fill(t::HOVER)
                    .wrap_mode(egui::TextWrapMode::Truncate),
            )
            .on_hover_text("Voir l’aperçu de la session")
            .clicked()
        {
            return Some(Intent::Preview(seat.id.clone()));
        }
        if ui
            .put(
                Rect::from_min_size(
                    pos2(
                        inner.left() + 86.0_f32.min(inner.width() * 0.4 + 6.0),
                        r.bottom() - 40.0,
                    ),
                    vec2(
                        (inner.width() - 86.0_f32.min(inner.width() * 0.4 + 6.0)).max(20.0),
                        28.0,
                    ),
                ),
                egui::Button::new("Fermer la session")
                    .fill(t::HOVER)
                    .wrap_mode(egui::TextWrapMode::Truncate),
            )
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
            color: t::SECONDARY,
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
                color: t::SECONDARY,
            });
        }
        list.push(Meter {
            name: "Encodeur".into(),
            value: format!("{} %", gpu.encoder_utilization_percent),
            ratio: gpu.encoder_utilization_percent as f32 / 100.0,
            color: t::SECONDARY,
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
            color: t::SECONDARY,
        });
    }
    list
}
fn ring(p: &egui::Painter, center: egui::Pos2, m: &Meter) {
    let radius = 30.0;
    p.circle_stroke(center, radius, Stroke::new(5.0_f32, t::HOVER));
    let steps = ((m.ratio.clamp(0.0, 1.0) * 72.0).ceil() as usize).max(1);
    if m.ratio > 0.0 {
        let points = (0..=steps)
            .map(|i| {
                let angle = -std::f32::consts::FRAC_PI_2
                    + std::f32::consts::TAU * m.ratio.clamp(0.0, 1.0) * i as f32 / steps as f32;
                center + vec2(angle.cos() * radius, angle.sin() * radius)
            })
            .collect::<Vec<_>>();
        p.add(egui::Shape::line(
            points,
            Stroke::new(5.0_f32, if m.ratio >= 0.9 { t::WARN } else { m.color }),
        ));
    }
    p.text(
        center,
        Align2::CENTER_CENTER,
        &m.value,
        t::title_font(13.0),
        t::PRIMARY,
    );
    p.text(
        center + vec2(0.0, 46.0),
        Align2::CENTER_CENTER,
        &m.name,
        FontId::proportional(12.0),
        t::SECONDARY,
    );
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
            ui.add_space(16.0);
            let (state, color) = views::health_label(&health.health.status);
            views::badge(ui, state, color);
        }
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
        views::empty(ui, "Aucune place configurée", "Aucune place n’est disponible. Vérifiez la configuration du service ; les places apparaîtront ici dès leur création.");
    } else {
        let cols = if width >= 780.0 {
            4
        } else if width >= 420.0 {
            2
        } else {
            1
        };
        let gap = 12.0;
        let w = ((width - gap * (cols - 1) as f32) / cols as f32).max(120.0);
        let height = if width < 520.0 { 174.0 } else { 240.0 };
        let rows = total.div_ceil(cols);
        // Réserver chaque rangée dans le flux vertical : les cadres suivants
        // commencent après la dernière rangée, même quand elle est hors écran.
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
    let list = meters(health);
    let issues = &health.health.issues;
    let two_panels = width >= 720.0 && !issues.is_empty();
    let panel_width = if two_panels {
        (width - 12.0) * 0.56
    } else {
        width
    };
    let columns = if panel_width >= 500.0 {
        5
    } else if panel_width >= 390.0 {
        3
    } else {
        2
    };
    let rows = list.len().div_ceil(columns);
    let resource_height = (76.0 + rows as f32 * 108.0).max(150.0);
    let (area, _) = ui.allocate_exact_size(
        vec2(
            width,
            if two_panels {
                resource_height.max(90.0 + issues.len() as f32 * 84.0)
            } else {
                resource_height
            },
        ),
        Sense::hover(),
    );
    let panel = Rect::from_min_size(area.min, vec2(panel_width, resource_height));
    let p = ui.painter();
    p.rect_filled(panel, CornerRadius::same(10), t::SURFACE);
    label(
        p,
        panel.left() + 16.0,
        panel.top() + 14.0,
        "Ressources du PC",
        17.0,
        t::PRIMARY,
        panel.width() - 32.0,
    );
    if list.is_empty() {
        label(
            p,
            panel.left() + 16.0,
            panel.top() + 54.0,
            "Mesures indisponibles sur ce PC",
            13.0,
            t::SECONDARY,
            panel.width() - 32.0,
        );
    } else {
        let step = (panel.width() - 32.0) / columns as f32;
        for (i, m) in list.iter().enumerate() {
            ring(
                p,
                pos2(
                    panel.left() + 16.0 + step * (i % columns) as f32 + step / 2.0,
                    panel.top() + 86.0 + (i / columns) as f32 * 112.0,
                ),
                m,
            );
        }
    }
    if two_panels {
        let alert = Rect::from_min_max(pos2(panel.right() + 12.0, area.top()), area.max);
        draw_issues(p, alert, issues);
    }
    if !two_panels && !issues.is_empty() {
        ui.add_space(12.0);
        let (alert, _) = ui.allocate_exact_size(
            vec2(width, 55.0 + issues.len() as f32 * 84.0),
            Sense::hover(),
        );
        draw_issues(ui.painter(), alert, issues);
    }
    ui.add_space(12.0);
    if ui.button("Voir toutes les places").clicked() {
        intent = Some(Intent::Seats);
    }
    intent
}
fn draw_issues(p: &egui::Painter, r: Rect, issues: &[String]) {
    p.rect_filled(r, CornerRadius::same(10), t::SURFACE);
    label(
        p,
        r.left() + 16.0,
        r.top() + 14.0,
        "À surveiller",
        17.0,
        t::PRIMARY,
        r.width() - 32.0,
    );
    for (i, message) in issues.iter().enumerate() {
        let (title, advice) = views::issue(message);
        let y = r.top() + 52.0 + i as f32 * 84.0;
        label(
            p,
            r.left() + 16.0,
            y,
            &title,
            13.0,
            t::WARN,
            r.width() - 32.0,
        );
        label(
            p,
            r.left() + 16.0,
            y + 25.0,
            advice,
            12.0,
            t::SECONDARY,
            r.width() - 32.0,
        );
    }
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
