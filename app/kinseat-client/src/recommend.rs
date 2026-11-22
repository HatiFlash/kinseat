//! Profil de démonstration : la géométrie du flux suit toujours l'écran client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenSpec {
    pub width: u32,
    pub height: u32,
    pub hz: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decoder {
    Av1,
    Hevc,
    H264,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_mbps: u32,
    pub codec: &'static str,
    pub explanation: String,
}
pub fn ratio(screen: ScreenSpec) -> &'static str {
    if screen.height == 0 {
        return "inconnu";
    }
    let r = screen.width as f64 / screen.height as f64;
    [
        (16.0 / 9.0, "16:9"),
        (16.0 / 10.0, "16:10"),
        (21.0 / 9.0, "21:9"),
    ]
    .into_iter()
    .min_by(|a, b| (a.0 - r).abs().total_cmp(&(b.0 - r).abs()))
    .filter(|(v, _)| (v - r).abs() < 0.06)
    .map(|(_, name)| name)
    .unwrap_or("autre")
}
pub fn recommend(
    screen: ScreenSpec,
    measured_mbps: Option<u32>,
    latency_ms: Option<u32>,
    decoder: Decoder,
) -> Profile {
    let width = screen.width.max(640);
    let height = screen.height.max(360);
    let bandwidth = measured_mbps.unwrap_or(40);
    let slow = bandwidth < 12 || latency_ms.is_some_and(|v| v > 75);
    let fps = screen.hz.clamp(
        30,
        if slow {
            30
        } else if bandwidth < 25 {
            60
        } else {
            120
        },
    );
    let bitrate_mbps = bandwidth.saturating_mul(7).saturating_div(10).clamp(1, 80);
    let codec = match decoder {
        Decoder::Av1 => "AV1",
        Decoder::Hevc => "HEVC",
        _ => "H.264",
    };
    // Une baisse de définition n'est qu'une proposition, après débit puis cadence.
    let explanation = if slow {
        format!("Écran natif {width} × {height} ({}) conservé ; débit et cadence réduits. Si nécessaire, choisir une définition inférieure au même ratio.", ratio(screen))
    } else {
        format!(
            "Écran natif {width} × {height} ({}) ; aucun changement de ratio.",
            ratio(screen)
        )
    };
    Profile {
        width,
        height,
        fps,
        bitrate_mbps,
        codec,
        explanation,
    }
}
pub fn same_ratio_options(screen: ScreenSpec) -> Vec<(u32, u32)> {
    let mut out = vec![(screen.width, screen.height)];
    for factor in [0.75_f64, 0.5] {
        let w = ((screen.width as f64 * factor / 2.0).round() as u32) * 2;
        let h = ((screen.height as f64 * factor / 2.0).round() as u32) * 2;
        if w >= 640 && h >= 360 && !out.contains(&(w, h)) {
            out.push((w, h));
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_ratios() {
        for (w, h, kind) in [
            (2560, 1600, "16:10"),
            (1920, 1080, "16:9"),
            (3440, 1440, "21:9"),
        ] {
            let s = ScreenSpec {
                width: w,
                height: h,
                hz: 60,
            };
            let p = recommend(s, Some(60), Some(12), Decoder::Hevc);
            assert_eq!((p.width, p.height), (w, h));
            assert_eq!(ratio(s), kind);
            for (a, b) in same_ratio_options(s) {
                assert!(((a as f64 / b as f64) - (w as f64 / h as f64)).abs() < 0.003);
            }
        }
    }
    #[test]
    fn weak_network_keeps_native_first() {
        let s = ScreenSpec {
            width: 2560,
            height: 1600,
            hz: 144,
        };
        let p = recommend(s, Some(5), Some(90), Decoder::Unknown);
        assert_eq!(
            (p.width, p.height, p.fps, p.codec),
            (2560, 1600, 30, "H.264")
        );
        assert_eq!(same_ratio_options(s)[1], (1920, 1200));
    }
}
