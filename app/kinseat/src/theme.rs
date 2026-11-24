use egui::{Color32, CornerRadius, FontFamily, FontId, RichText, Stroke, Style, Visuals};

pub const CANVAS: Color32 = Color32::from_rgb(0x13, 0x12, 0x11);
pub const RAIL: Color32 = Color32::from_rgb(0x19, 0x18, 0x16);
pub const SURFACE: Color32 = Color32::from_rgb(0x1f, 0x1d, 0x1b);
pub const HOVER: Color32 = Color32::from_rgb(0x2a, 0x26, 0x22);
pub const BORDER: Color32 = Color32::from_rgb(0x33, 0x2f, 0x2b);
pub const PRIMARY: Color32 = Color32::from_rgb(0xef, 0xea, 0xe4);
pub const SECONDARY: Color32 = Color32::from_rgb(0xac, 0xa4, 0x9a);
pub const FAINT: Color32 = Color32::from_rgb(0x83, 0x7b, 0x72);
pub const ACCENT: Color32 = Color32::from_rgb(0xe8, 0xa3, 0x3d);
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x1a, 0x12, 0x06);
pub const OK: Color32 = Color32::from_rgb(0x6c, 0xc7, 0x88);
pub const WARN: Color32 = Color32::from_rgb(0xf0, 0x8a, 0x4b);
pub const ERR: Color32 = Color32::from_rgb(0xef, 0x6a, 0x60);
pub const MEMBERS: [Color32; 6] = [
    Color32::from_rgb(0x7f, 0xa7, 0xc9),
    Color32::from_rgb(0xc9, 0x9a, 0x7f),
    Color32::from_rgb(0x9a, 0xc9, 0x7f),
    Color32::from_rgb(0xc9, 0x7f, 0xa4),
    Color32::from_rgb(0x7f, 0xc9, 0xbe),
    Color32::from_rgb(0xb7, 0xa6, 0xd9),
];

pub fn member_color(name: &str) -> Color32 {
    let hash = name.bytes().fold(2166136261_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16777619)
    });
    MEMBERS[hash as usize % MEMBERS.len()]
}

pub fn text(value: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(value).size(size).color(color)
}
/// Police des titres : Segoe UI Semibold sous Windows (si chargée), sinon la police proportionnelle.
pub fn title_family() -> FontFamily {
    if cfg!(windows) && crate::fonts::has_titles() {
        FontFamily::Name("Titres".into())
    } else {
        FontFamily::Proportional
    }
}
pub fn title_font(size: f32) -> FontId {
    FontId::new(size, title_family())
}
pub fn title(value: impl Into<String>, size: f32) -> RichText {
    text(value, size, PRIMARY).family(title_family())
}
pub fn pulse(ctx: &egui::Context, status: &str, reduced: bool, color: Color32) -> Color32 {
    if reduced
        || !matches!(
            status,
            "Streaming" | "Provisioning" | "Configuring" | "Connecting"
        )
    {
        return color;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(33));
    let phase = (ctx.input(|i| i.time) * std::f64::consts::TAU / 1.6).sin() as f32;
    let opacity = (0.75 + 0.25 * phase) * 255.0;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), opacity as u8)
}

pub fn card_hover(
    ui: &egui::Ui,
    id: egui::Id,
    hovered: bool,
    reduced: bool,
    rect: egui::Rect,
) -> egui::Rect {
    let amount = if reduced {
        f32::from(hovered)
    } else {
        ui.ctx().animate_bool_with_time(id, hovered, 0.15)
    };
    let raised = rect.translate(egui::vec2(0.0, -3.0 * amount));
    ui.painter().rect_filled(
        raised,
        CornerRadius::same(10),
        SURFACE.lerp_to_gamma(HOVER, amount * 0.45),
    );
    ui.painter().rect_stroke(
        raised,
        CornerRadius::same(10),
        Stroke::new(1.0_f32, BORDER.lerp_to_gamma(SECONDARY, amount * 0.7)),
        egui::StrokeKind::Inside,
    );
    raised
}

pub fn frame() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(12)
        .inner_margin(16)
}
pub fn install(ctx: &egui::Context) {
    // Kinseat est toujours sombre, quel que soit le thème de Windows : sinon egui suit le
    // thème système et applique ses visuels clairs aux fenêtres, boutons et champs.
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut visuals = Visuals::dark();
    visuals.panel_fill = CANVAS;
    visuals.window_fill = SURFACE;
    visuals.widgets.noninteractive.bg_fill = SURFACE;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, PRIMARY);
    // Séparateurs et bords de panneaux : discrets, jamais blancs.
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.extreme_bg_color = CANVAS;
    visuals.faint_bg_color = SURFACE;
    visuals.override_text_color = None;
    visuals.widgets.inactive.weak_bg_fill = HOVER;
    visuals.widgets.hovered.weak_bg_fill = BORDER;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.inactive.bg_fill = HOVER;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, PRIMARY);
    visuals.widgets.hovered.bg_fill = BORDER;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, PRIMARY);
    visuals.widgets.active.bg_fill = ACCENT;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, ON_ACCENT);
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = Stroke::new(1.0_f32, ON_ACCENT);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
    visuals.widgets.active.corner_radius = CornerRadius::same(6);
    visuals.window_shadow = egui::epaint::Shadow::NONE;
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.text_edit_bg_color = Some(CANVAS);
    visuals.popup_shadow = egui::epaint::Shadow::NONE;
    ctx.set_visuals_of(egui::Theme::Dark, visuals);
    let mut style: Style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.spacing.window_margin = egui::Margin::same(24);
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 8.0);
    for (kind, size) in [
        (egui::TextStyle::Body, 14.0),
        (egui::TextStyle::Button, 14.0),
        (egui::TextStyle::Small, 12.0),
        (egui::TextStyle::Heading, 21.0),
    ] {
        style
            .text_styles
            .insert(kind, FontId::new(size, FontFamily::Proportional));
    }
    ctx.set_style_of(egui::Theme::Dark, style);
}

/// Pastille de choix : sélectionnée = texte sombre sur ACCENT plein (contraste élevé).
pub fn choice_button(selected: bool, label: impl Into<String>) -> egui::Button<'static> {
    let label: String = label.into();
    if selected {
        egui::Button::new(egui::RichText::new(label).color(ON_ACCENT)).fill(ACCENT)
    } else {
        egui::Button::new(label)
    }
}

/// Touches clavier d'une modale : `esc` = annuler/fermer, `enter` = action principale.
#[derive(Clone, Copy, Default)]
pub struct ModalKeys {
    pub esc: bool,
    pub enter: bool,
}

/// Lit Échap/Entrée une seule fois par frame, uniquement pour la modale au premier plan
/// (fenêtre `id`). Entrée est ignorée si un bouton a le focus clavier (il gère déjà Entrée)
/// ou si `allow_enter` est faux (action destructrice, champ multi-ligne actif).
/// L'appelant doit en plus vérifier que le bouton principal est actif.
pub fn modal_keys(ctx: &egui::Context, id: egui::Id, allow_enter: bool) -> ModalKeys {
    let layer = egui::LayerId::new(egui::Order::Middle, id);
    // Un clic sur un voile (scrim) le remonte au-dessus de sa fenêtre : il ne masque pas la modale.
    let scrims = ["join-request-scrim", "preview-scrim", "add-seat-scrim", "schedule-scrim"]
        .map(|name| egui::LayerId::new(egui::Order::Middle, egui::Id::new(name)));
    let blocked = ctx.memory(|m| match m.areas().top_layer_id(egui::Order::Middle) {
        Some(top) => top != layer && !scrims.contains(&top),
        None => false,
    });
    if blocked || egui::Popup::is_any_open(ctx) {
        return ModalKeys::default();
    }
    let button_focused = ctx.memory(|m| m.focused().is_some()) && !ctx.wants_keyboard_input();
    ctx.input_mut(|i| ModalKeys {
        esc: i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
        enter: allow_enter && !button_focused && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
    })
}
