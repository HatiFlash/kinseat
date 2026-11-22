use std::sync::atomic::{AtomicBool, Ordering};
static TITLES: AtomicBool = AtomicBool::new(false);
pub fn has_titles() -> bool {
    TITLES.load(Ordering::Relaxed)
}
pub fn install(ctx: &egui::Context) {
    #[cfg(windows)]
    {
        use egui::{FontData, FontDefinitions, FontFamily};
        let regular = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf");
        let semibold = std::fs::read(r"C:\Windows\Fonts\seguisb.ttf");
        if let (Ok(regular), Ok(semibold)) = (regular, semibold) {
            let mut defs = FontDefinitions::default();
            defs.font_data.insert(
                "segoe".into(),
                std::sync::Arc::new(FontData::from_owned(regular)),
            );
            defs.font_data.insert(
                "segoe-semibold".into(),
                std::sync::Arc::new(FontData::from_owned(semibold)),
            );
            defs.families
                .get_mut(&FontFamily::Proportional)
                .unwrap()
                .insert(0, "segoe".into());
            defs.families.insert(
                FontFamily::Name("Titres".into()),
                vec!["segoe-semibold".into(), "segoe".into()],
            );
            ctx.set_fonts(defs);
            TITLES.store(true, Ordering::Relaxed);
        }
    }
    #[cfg(not(windows))]
    let _ = ctx;
}
