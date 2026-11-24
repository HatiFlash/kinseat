use crate::theme;
use kinseat_ipc::{GpuBudget, GraphicsChoice};

pub fn profile_label(value: &str) -> &'static str {
    match value {
        "Economy" => "Économie",
        "Quality" => "Qualité",
        "Custom" => "Personnalisé",
        _ => "Équilibré",
    }
}

pub fn encoding_label(value: &str) -> &'static str {
    match value {
        "Quality" => "Qualité",
        "Latency" => "Faible latence",
        _ => "Équilibré",
    }
}

pub fn selector(ui: &mut egui::Ui, id: impl std::hash::Hash, choice: &mut GraphicsChoice) {
    ui.label(theme::title("Profil graphique", 15.0));
    egui::ComboBox::from_id_salt(id)
        .selected_text(profile_label(&choice.graphics_profile))
        .show_ui(ui, |ui| {
            for value in ["Economy", "Balanced", "Quality", "Custom"] {
                if ui
                    .selectable_label(choice.graphics_profile == value, profile_label(value))
                    .clicked()
                {
                    *choice = GraphicsChoice::preset(value);
                    if value == "Custom" {
                        choice.vram_budget_mb = Some(3072);
                        choice.width = Some(1920);
                        choice.height = Some(1080);
                        choice.fps = Some(60);
                        choice.nvenc_preset = Some("Balanced".into());
                    }
                }
            }
        });
    if choice.graphics_profile == "Custom" {
        let mut budget = choice.budget_mb();
        let mut width = choice.width.unwrap_or(1920);
        let mut height = choice.height.unwrap_or(1080);
        let mut fps = choice.fps.unwrap_or(60);
        let mut preset = choice
            .nvenc_preset
            .clone()
            .unwrap_or_else(|| "Balanced".into());
        egui::Grid::new("custom-graphics-grid")
            .num_columns(2)
            .spacing([14.0, 6.0])
            .show(ui, |ui| {
                ui.label("Budget (Mo)")
                    .on_hover_text("Budget indicatif de VRAM pour cette place.");
                ui.add(
                    egui::DragValue::new(&mut budget)
                        .range(512..=65536)
                        .speed(128),
                );
                ui.end_row();
                ui.label("Largeur")
                    .on_hover_text("Largeur de l'écran en pixels.");
                ui.add(egui::DragValue::new(&mut width).range(640..=7680));
                ui.end_row();
                ui.label("Hauteur")
                    .on_hover_text("Hauteur de l'écran en pixels.");
                ui.add(egui::DragValue::new(&mut height).range(480..=4320));
                ui.end_row();
                ui.label("i/s").on_hover_text("Images par seconde du flux.");
                ui.add(egui::DragValue::new(&mut fps).range(30..=240));
                ui.end_row();
                ui.label("Encodage")
                    .on_hover_text("Qualité ou réactivité du flux vidéo.");
                egui::ComboBox::from_id_salt("custom-nvenc")
                    .selected_text(encoding_label(&preset))
                    .show_ui(ui, |ui| {
                        for value in ["Quality", "Balanced", "Latency"] {
                            ui.selectable_value(&mut preset, value.into(), encoding_label(value));
                        }
                    });
                ui.end_row();
            });
        choice.vram_budget_mb = Some(budget);
        choice.width = Some(width);
        choice.height = Some(height);
        choice.fps = Some(fps);
        choice.nvenc_preset = Some(preset);
    }
    ui.label(theme::text(
        format!("Budget indicatif : {} Mo", choice.budget_mb()),
        13.0,
        theme::SECONDARY,
    ));
}

pub fn gauge(
    ui: &mut egui::Ui,
    budget: Option<&GpuBudget>,
    choice: &GraphicsChoice,
    reduced: bool,
) {
    if let Some(budget) = budget {
        ui.label(theme::text(format!("{} : {} Mo libres sur {} Mo attribuables aux places ({} Mo gardés pour le poste principal)", budget.gpu_name, budget.free_mb, budget.usable_mb, budget.host_reserve_mb), 13.0, theme::PRIMARY));
        let over = choice.budget_mb() > budget.free_mb;
        let ratio = if over {
            1.0
        } else if budget.free_mb == 0 {
            0.0
        } else {
            choice.budget_mb() as f32 / budget.free_mb as f32
        };
        let target = ratio.clamp(0.0, 1.0);
        let value = if reduced {
            target
        } else {
            ui.ctx()
                .animate_value_with_time(ui.id().with("vram-gauge"), target, 0.24)
        };
        ui.add(
            egui::ProgressBar::new(value)
                .fill(if over { theme::ERR } else { theme::ACCENT })
                .desired_width(340.0),
        )
        .on_hover_text("Budget demandé par rapport à la VRAM libre");
        if choice.budget_mb() > budget.free_mb {
            ui.label(theme::text("Le budget choisi dépasse la VRAM disponible. Réduisez le profil, libérez une place ou diminuez la réserve du poste principal.", 13.0, theme::WARN));
        }
    } else {
        ui.label(theme::text(
            "Budget VRAM indisponible : vérifiez la connexion ou mettez à jour le service.",
            13.0,
            theme::SECONDARY,
        ));
    }
    ui.label(theme::text("Windows ne réserve pas la VRAM : le budget est indicatif. L'utilisation réelle est mesurée et un dépassement est signalé.", 12.0, theme::FAINT));
}

pub fn error_text(error: &str) -> String {
    if error.starts_with("insufficient_vram") {
        format!("VRAM insuffisante pour ce profil. {error} Réduisez le budget, libérez une place ou ajustez la réserve du poste principal.")
    } else if error.starts_with("unknown_method") {
        "Mettez à jour le service Kinseat".into()
    } else {
        crate::notice::seat_error(error)
    }
}
