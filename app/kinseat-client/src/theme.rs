use egui::{vec2, Color32, CornerRadius, FontFamily, FontId, Rect, Stroke, Style, Visuals};

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
pub const ERR: Color32 = Color32::from_rgb(0xef, 0x6a, 0x60);
pub const MEMBERS: [Color32; 6] = [
    Color32::from_rgb(0x7f, 0xa7, 0xc9),
    Color32::from_rgb(0xc9, 0x9a, 0x7f),
    Color32::from_rgb(0x9a, 0xc9, 0x7f),
    Color32::from_rgb(0xc9, 0x7f, 0xa4),
    Color32::from_rgb(0x7f, 0xc9, 0xbe),
    Color32::from_rgb(0xb7, 0xa6, 0xd9),
];

pub fn title_family() -> FontFamily {
    if cfg!(windows) {
        FontFamily::Name("Titres".into())
    } else {
        FontFamily::Proportional
    }
}

/// Emblème Kinseat à deux pilules ambrées (identique à Kinseat Hôte).
pub fn draw_brand_mark(p: &egui::Painter, origin: egui::Pos2, scale: f32) {
    let left = Rect::from_min_size(
        origin + vec2(0.0, 3.0 * scale),
        vec2(7.0 * scale, 13.0 * scale),
    );
    let right = Rect::from_min_size(
        origin + vec2(10.0 * scale, 0.0),
        vec2(7.0 * scale, 18.0 * scale),
    );
    let r = CornerRadius::same((2.5 * scale).round() as u8);
    p.rect_filled(left, r, ACCENT);
    p.rect_filled(right, r, ACCENT.gamma_multiply(0.72));
}

pub fn install(ctx: &egui::Context) {
    #[cfg(windows)]
    {
        use egui::{FontData, FontDefinitions};
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let regular_path = format!(r"{windir}\Fonts\segoeui.ttf");
        let semibold_path = format!(r"{windir}\Fonts\seguisb.ttf");
        if let (Ok(regular), Ok(semibold)) =
            (std::fs::read(regular_path), std::fs::read(semibold_path))
        {
            let mut fonts = FontDefinitions::default();
            fonts.font_data.insert(
                "segoe".into(),
                std::sync::Arc::new(FontData::from_owned(regular)),
            );
            fonts.font_data.insert(
                "segoe-semibold".into(),
                std::sync::Arc::new(FontData::from_owned(semibold)),
            );
            if let Some(prop) = fonts.families.get_mut(&FontFamily::Proportional) {
                prop.insert(0, "segoe".into());
            }
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
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, PRIMARY);
    v.widgets.hovered.bg_fill = BORDER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, PRIMARY);
    v.widgets.active.bg_fill = ACCENT;
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, ON_ACCENT);
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
