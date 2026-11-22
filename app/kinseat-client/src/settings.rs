use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub resolution: String,
    pub custom_width: u32,
    pub custom_height: u32,
    pub fps: String,
    pub automatic_bitrate: bool,
    pub bitrate: u32,
    pub codec: String,
    pub decoder: String,
    pub hdr: bool,
    pub yuv444: bool,
    pub vsync: bool,
    pub frame_pacing: bool,
    pub display_mode: String,
    pub stream_menu_shortcut: String,
    pub target_display: String,
    pub audio: String,
    pub mute_host: bool,
    pub play_on_host: bool,
    pub keyboard_capture: String,
    pub remote_mouse: bool,
    pub invert_mouse: bool,
    pub invert_scroll: bool,
    pub swap_gamepad: bool,
    pub multiple_gamepads: bool,
    pub gamepad_mouse: bool,
    pub background_gamepad: bool,
    pub shortcuts: Vec<String>,
    pub osd: bool,
    pub osd_position: String,
    pub immersive: bool,
    pub dashboard: bool,
    /// Empêche la mise en veille de l'écran pendant le flux (Moonlight `--keep-awake`).
    pub keep_awake: bool,
    /// Laisse l'hôte adapter les réglages du jeu au flux (`--game-optimization`).
    pub game_optimization: bool,
    /// Ferme le programme de la place quand on quitte le flux (`--quit-after`).
    pub quit_after: bool,
    pub connection_warnings: bool,
    pub mute_focus: bool,
    pub engine_path: String,
    pub device_name: String,
    pub linked_place: Option<crate::discovery::Place>,
    pub app_name: String,
    /// Profil du joueur (0.3.8) : pseudo, PIN en hash salé, identifiant d'appareil stable.
    pub pseudo: String,
    pub pin_salt: String,
    pub pin_hash: String,
    pub device_id: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            resolution: "Automatique".into(),
            custom_width: 1920,
            custom_height: 1080,
            fps: "Automatique".into(),
            automatic_bitrate: true,
            bitrate: 40,
            codec: "Automatique".into(),
            decoder: "Automatique".into(),
            hdr: false,
            yuv444: false,
            vsync: true,
            frame_pacing: true,
            display_mode: "Plein écran".into(),
            stream_menu_shortcut: "Alt+K".into(),
            target_display: "Automatique".into(),
            audio: "Stéréo".into(),
            mute_host: false,
            play_on_host: false,
            keyboard_capture: "En plein écran seulement".into(),
            remote_mouse: false,
            invert_mouse: false,
            invert_scroll: false,
            swap_gamepad: false,
            multiple_gamepads: true,
            gamepad_mouse: false,
            background_gamepad: false,
            shortcuts: ["M", "Q", "S", "Z", "X", "D", "V"]
                .map(|k| format!("Ctrl+Alt+Maj+{k}"))
                .to_vec(),
            osd: false,
            osd_position: "Haut droit".into(),
            immersive: false,
            dashboard: false,
            keep_awake: true,
            game_optimization: true,
            quit_after: false,
            connection_warnings: true,
            mute_focus: false,
            engine_path: String::new(),
            device_name: String::new(),
            linked_place: None,
            app_name: "Desktop".into(),
            pseudo: String::new(),
            pin_salt: String::new(),
            pin_hash: String::new(),
            device_id: String::new(),
        }
    }
}
pub fn config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|v| PathBuf::from(v).join(".config")));
    base.map(|v| v.join("kinseat").join("client.json"))
}
impl Settings {
    pub fn load() -> Self {
        let mut settings: Self = config_path()
            .and_then(|p| fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        if settings.stream_menu_shortcut == "Alt+Z" {
            settings.stream_menu_shortcut = "Alt+K".into();
        }
        settings
    }
    pub fn save(&self) -> std::io::Result<()> {
        let path = config_path()
            .ok_or_else(|| std::io::Error::other("Dossier de configuration absent"))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(tmp, path)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linked_place_retains_session_access() {
        let settings = Settings {
            linked_place: Some(crate::discovery::Place {
                name: "Salon".into(),
                address: "192.0.2.10".into(),
                port: 48100,
                session_access: Some(crate::discovery::SessionAccess {
                    request_id: "request-123".into(),
                    token: "token-0123456789abcdef".into(),
                    address: "192.0.2.11".into(),
                    port: 47960,
                }),
            }),
            ..Settings::default()
        };
        let encoded = serde_json::to_string(&settings).unwrap();
        let restored: Settings = serde_json::from_str(&encoded).unwrap();
        assert_eq!(restored.linked_place, settings.linked_place);
        assert!(!format!("{:?}", restored.linked_place).contains("token-0123456789abcdef"));
        let old = serde_json::json!({"linked_place": {"name": "Salon", "address": "192.0.2.10", "port": 48100}});
        assert!(serde_json::from_value::<Settings>(old)
            .unwrap()
            .linked_place
            .unwrap()
            .session_access
            .is_none());
    }

    #[test]
    fn roundtrip_defaults_and_conflict() {
        let original = Settings::default();
        let encoded = serde_json::to_string(&original).unwrap();
        assert_eq!(
            serde_json::from_str::<Settings>(&encoded).unwrap(),
            original
        );
        assert_eq!(original.resolution, "Automatique");
        let mut changed = original;
        changed.shortcuts[1] = changed.shortcuts[0].clone();
    }
}
