use egui::{Color32, FontFamily, FontId, Stroke, Style, Visuals};

pub const CANVAS: Color32 = Color32::from_rgb(0x15, 0x14, 0x13);
pub const RAIL: Color32 = Color32::from_rgb(0x1c, 0x1a, 0x18);
pub const SURFACE: Color32 = Color32::from_rgb(0x23, 0x21, 0x1e);
pub const HOVER: Color32 = Color32::from_rgb(0x2e, 0x2a, 0x26);
pub const BORDER: Color32 = Color32::from_rgb(0x3a, 0x36, 0x32);
pub const PRIMARY: Color32 = Color32::from_rgb(0xed, 0xe8, 0xe1);
pub const SECONDARY: Color32 = Color32::from_rgb(0xb3, 0xab, 0xa1);
pub const FAINT: Color32 = Color32::from_rgb(0x94, 0x8c, 0x82);
pub const ACCENT: Color32 = Color32::from_rgb(0xe8, 0xa3, 0x3d);
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x1a, 0x12, 0x06);
pub const OK: Color32 = Color32::from_rgb(0x6c, 0xc7, 0x88);
pub const ERR: Color32 = Color32::from_rgb(0xef, 0x6a, 0x60);
pub const MEMBERS: [Color32; 3] = [
    Color32::from_rgb(0x7f, 0xa7, 0xc9),
    Color32::from_rgb(0xc9, 0x9a, 0x7f),
    Color32::from_rgb(0x9a, 0xc9, 0x7f),
];

pub fn install(ctx: &egui::Context) {
    #[cfg(windows)]
    {
        use egui::{FontData, FontDefinitions};
        if let (Ok(regular), Ok(semibold)) = (
            std::fs::read(r"C:\Windows\Fonts\segoeui.ttf"),
            std::fs::read(r"C:\Windows\Fonts\seguisb.ttf"),
        ) {
            let mut fonts = FontDefinitions::default();
            fonts.font_data.insert(
                "segoe".into(),
                std::sync::Arc::new(FontData::from_owned(regular)),
            );
            fonts.font_data.insert(
                "segoe-semibold".into(),
                std::sync::Arc::new(FontData::from_owned(semibold)),
            );
            fonts
                .families
                .get_mut(&FontFamily::Proportional)
                .unwrap()
                .insert(0, "segoe".into());
            fonts.families.insert(
                FontFamily::Name("Titres".into()),
                vec!["segoe-semibold".into(), "segoe".into()],
            );
            ctx.set_fonts(fonts);
        }
    }
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut v = Visuals::dark();
    v.panel_fill = CANVAS;
    v.window_fill = SURFACE;
    v.extreme_bg_color = CANVAS;
    v.faint_bg_color = SURFACE;
    v.widgets.noninteractive.bg_fill = SURFACE;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.inactive.bg_fill = HOVER;
    v.widgets.inactive.weak_bg_fill = HOVER;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.bg_fill = BORDER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    v.widgets.active.bg_fill = ACCENT;
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0_f32, ON_ACCENT);
    v.window_shadow = egui::epaint::Shadow::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;
    v.text_edit_bg_color = Some(CANVAS);
    ctx.set_visuals_of(egui::Theme::Dark, v);
    let mut style: Style = (*ctx.style_of(egui::Theme::Dark)).clone();
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

/// Avatar dessiné pour Kinseat (même silhouette que l’hôte) : stable par membre,
/// sans prétendre représenter le visage réel de la personne.
pub fn draw_avatar(p: &egui::Painter, center: egui::Pos2, radius: f32, color: Color32) {
    use egui::{vec2, Stroke};
    let k = radius / 21.0;
    p.circle_filled(center, radius, color);
    let shoulders: Vec<egui::Pos2> = (0..=24)
        .map(|i| {
            let a = std::f32::consts::PI * (1.0 + i as f32 / 24.0);
            center + vec2(12.0 * k * a.cos(), (15.0 + 9.0 * a.sin()) * k)
        })
        .collect();
    p.add(egui::Shape::convex_polygon(
        shoulders,
        PRIMARY.gamma_multiply(0.85),
        Stroke::NONE,
    ));
    p.circle_filled(center + vec2(0.0, -4.0 * k), 7.5 * k, PRIMARY);
    if radius >= 16.0 {
        p.circle_filled(center + vec2(-2.6 * k, -4.5 * k), 1.1 * k, ON_ACCENT);
        p.circle_filled(center + vec2(2.6 * k, -4.5 * k), 1.1 * k, ON_ACCENT);
        p.line_segment(
            [
                center + vec2(-1.8 * k, -1.2 * k),
                center + vec2(1.8 * k, -1.2 * k),
            ],
            Stroke::new((1.0 * k).max(1.0), ON_ACCENT),
        );
    }
}
