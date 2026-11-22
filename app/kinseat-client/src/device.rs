use crate::recommend::{ratio, ScreenSpec};
#[derive(Clone)]
pub struct Device {
    pub screen: ScreenSpec,
    pub scale: f32,
    pub cpu: String,
    pub ram: String,
    pub gpu: String,
    pub network: String,
}
impl Device {
    pub fn detect(ctx: &egui::Context) -> Self {
        let (screen, scale) = ctx.input(|i| {
            let info = i.viewport();
            let scale = info.native_pixels_per_point.unwrap_or(i.pixels_per_point());
            let size = info
                .monitor_size
                .filter(|size| size.x > 0.0 && size.y > 0.0 && size.x < 8192.0 && size.y < 8192.0)
                .unwrap_or_else(|| i.screen_rect.size());
            (
                ScreenSpec {
                    width: (size.x * scale).round() as u32,
                    height: (size.y * scale).round() as u32,
                    hz: 60,
                },
                scale,
            )
        });
        #[cfg(target_os = "linux")]
        let (cpu, ram) = {
            let cpu = std::fs::read_to_string("/proc/cpuinfo")
                .ok()
                .and_then(|s| {
                    s.lines().find_map(|l| {
                        l.strip_prefix("model name")
                            .and_then(|v| v.split_once(':'))
                            .map(|(_, v)| v.trim().to_owned())
                    })
                })
                .unwrap_or_else(|| "À détecter".into());
            let ram = std::fs::read_to_string("/proc/meminfo")
                .ok()
                .and_then(|s| {
                    s.lines().find_map(|l| {
                        l.strip_prefix("MemTotal:")
                            .and_then(|v| v.split_whitespace().next())
                            .and_then(|v| v.parse::<u64>().ok())
                    })
                })
                .map(|kb| format!("{} Gio", kb / 1024 / 1024))
                .unwrap_or_else(|| "À détecter".into());
            (cpu, ram)
        };
        #[cfg(not(target_os = "linux"))]
        let (cpu, ram) = (
            std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "À détecter".into()),
            "À détecter".into(),
        );
        Self {
            screen,
            scale,
            cpu,
            ram,
            gpu: "Bientôt détecté".into(),
            network: "Bientôt détecté".into(),
        }
    }
    pub fn screen_label(&self) -> String {
        format!(
            "{} × {} · {} · {} Hz estimés · échelle {:.0}%",
            self.screen.width,
            self.screen.height,
            ratio(self.screen),
            self.screen.hz,
            self.scale * 100.0
        )
    }
}
