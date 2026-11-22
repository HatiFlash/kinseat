//! Notice geometry in physical pixels; independent of the Win32 monitor APIs.
#[derive(Debug, PartialEq, Eq)]
pub struct NoticeLayout {
    pub width: i32,
    pub height: i32,
    pub radius: i32,
    pub margin: i32,
    pub text: i32,
    pub help: i32,
}

pub fn notice_layout(work_width: i32, dpi: i32) -> NoticeLayout {
    let width = ((work_width as f64 * 0.19).round() as i32)
        .clamp((300 * dpi / 96).min(460), 460)
        // A narrow high-DPI monitor cannot satisfy both a 300-DIP minimum
        // and the unobtrusive maximum: the screen fraction takes precedence.
        .min(work_width * 239 / 1000)
        .max(1);
    let factor = width as f64 / 300.0;
    let px = |n: f64| (n * factor).round() as i32;
    NoticeLayout {
        width,
        height: px(164.0),
        radius: px(16.0),
        margin: px(18.0),
        text: px(14.0).max(12 * dpi / 96),
        help: px(12.0).max(12 * dpi / 96),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_on_reference_monitors() {
        for (w, h, dpi) in [
            (1280, 720, 96),
            (1920, 1080, 96),
            (2560, 1440, 96),
            (3840, 2160, 144),
            (1366, 768, 120),
        ] {
            let panel = notice_layout(w, dpi);
            assert!(panel.width * 100 < w * 24, "{w}x{h}: {panel:?}");
            assert!(
                i64::from(panel.width) * i64::from(panel.height) * 100
                    < i64::from(w) * i64::from(h) * 6,
                "{w}x{h}: {panel:?}"
            );
            assert!(panel.text >= 12 * dpi / 96 && panel.help >= 12 * dpi / 96);
            println!("{w}x{h}@{dpi}: {}x{}", panel.width, panel.height);
        }
    }
}
