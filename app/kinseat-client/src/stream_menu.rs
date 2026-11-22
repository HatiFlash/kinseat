//! État et présentation du panneau de flux, sans état clavier persistant.
use crate::settings::Settings;
use std::time::Duration;

pub enum Action {
    Resume,
    Shortcuts,
    ConfirmQuit,
    CancelQuit,
    Key(u8),
    Quit,
}

/// Le raccourci Z de Moonlight est un toggle. Selon le mode (fenêtré, plein écran, souris
/// absolue), la perte de focus au profit du panneau a pu libérer la souris ou non : on ne
/// l'envoie que si la souris est encore réellement capturée (curseur confiné ou masqué).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn release_needs_toggle(cursor_clipped: bool, cursor_hidden: bool) -> bool {
    cursor_clipped || cursor_hidden
}

// Liste utilisée par les tests ; l'interface des réglages garde sa propre liste.
#[cfg(test)]
pub const SHORTCUTS: [&str; 3] = ["Alt+K", "Ctrl+Alt+Maj+K", "Désactivé"];

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Panel {
    pub open: bool,
    pub confirm_quit: bool,
    pub shortcuts: bool,
}
impl Panel {
    // L'ouverture au clavier n'existe que sur Windows ; les tests valident l'état ailleurs.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.confirm_quit = false;
        self.shortcuts = false;
    }
    pub fn close(&mut self) {
        self.open = false;
        self.confirm_quit = false;
        self.shortcuts = false;
    }
}

/// État issu des événements du crochet, pas de l'état asynchrone Windows (SDL peut avaler Alt).
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Default, Clone, Copy)]
pub struct ShortcutKeys {
    alt: [bool; 2],
    ctrl: [bool; 2],
    shift: [bool; 2],
    k: bool,
    captured_k: bool,
}

#[derive(Default, PartialEq, Eq, Debug)]
pub struct KeyDecision {
    pub swallow: bool,
    pub trigger: bool,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl ShortcutKeys {
    pub fn idle(&self) -> bool {
        !self.alt.iter().any(|&v| v) && !self.k
    }

    // VK_L/RMENU, VK_L/RCONTROL, VK_L/RSHIFT ; le code générique conserve
    // un emplacement distinct si un pilote ne distingue pas le côté.
    pub fn event(&mut self, shortcut: &str, vk: u32, down: bool, active: bool) -> KeyDecision {
        if !active {
            *self = Self::default();
            return KeyDecision::default();
        }
        match vk {
            0x12 | 0xA4 => self.alt[0] = down,
            0xA5 => self.alt[1] = down,
            0x11 | 0xA2 => self.ctrl[0] = down,
            0xA3 => self.ctrl[1] = down,
            0x10 | 0xA0 => self.shift[0] = down,
            0xA1 => self.shift[1] = down,
            _ => {}
        }
        if vk != b'K' as u32 {
            return KeyDecision::default();
        }
        let trigger = down
            && !self.k
            && matches_shortcut(
                shortcut,
                vk,
                self.ctrl.iter().any(|&v| v),
                self.alt.iter().any(|&v| v),
                self.shift.iter().any(|&v| v),
            );
        self.k = down;
        self.captured_k |= trigger;
        let swallow = self.captured_k;
        if !down {
            self.captured_k = false;
        }
        KeyDecision { swallow, trigger }
    }
}

/// Reconnaît Alt+K seul ou la combinaison de secours ; exclut AltGr et le changement de disposition.
// Appelée par le crochet Windows ; hors Windows elle n'est exercée que par les tests.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn matches_shortcut(shortcut: &str, vk: u32, ctrl: bool, alt: bool, shift: bool) -> bool {
    match shortcut {
        "Alt+K" => vk == b'K' as u32 && alt && !ctrl && !shift,
        "Ctrl+Alt+Maj+K" => vk == b'K' as u32 && ctrl && alt && shift,
        _ => false,
    }
}

pub fn display_mode(settings: &Settings) -> &'static str {
    match settings.display_mode.as_str() {
        "Fenêtré" => "windowed",
        "Fenêtré sans bordure" => "borderless",
        "Plein écran" if settings.stream_menu_shortcut != "Désactivé" => "borderless",
        _ => "fullscreen",
    }
}
/// Délai de grâce monotone avant de forcer l'arrêt du Moonlight lancé par le Client.
pub fn quit_deadline_reached(elapsed: Duration) -> bool {
    elapsed >= Duration::from_secs(5)
}

pub fn duration(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60
    )
}
pub fn latency(samples: &[u32]) -> String {
    if samples.is_empty() {
        return "indisponible".into();
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    format!("{} ms", sorted[sorted.len() / 2])
}
pub fn profile(settings: &Settings) -> String {
    format!(
        "{} · {} i/s · {} · {} · {}",
        settings.resolution,
        settings.fps,
        if settings.automatic_bitrate {
            "débit automatique".into()
        } else {
            format!("{} Mb/s", settings.bitrate)
        },
        settings.codec,
        settings.display_mode
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_mouse_release_toggles_only_a_captured_mouse() {
        assert!(!release_needs_toggle(false, false));
        assert!(release_needs_toggle(true, false));
        assert!(release_needs_toggle(false, true));
        assert!(release_needs_toggle(true, true));
    }
    #[test]
    fn panel_lifecycle() {
        let mut p = Panel::default();
        p.toggle();
        p.confirm_quit = true;
        p.shortcuts = true;
        p.toggle();
        assert_eq!(p, Panel::default());
    }
    #[test]
    fn combinations() {
        for shortcut in SHORTCUTS {
            assert!(
                matches_shortcut(
                    shortcut,
                    b'K' as u32,
                    shortcut.contains("Ctrl"),
                    true,
                    shortcut.contains("Maj")
                ) || shortcut == "Désactivé"
            );
        }
        assert!(matches_shortcut("Alt+K", 75, false, true, false)); // Alt gauche ou droit
        assert!(!matches_shortcut("Alt+K", 75, true, true, false)); // AltGr
        assert!(!matches_shortcut("Alt+K", 75, false, true, true)); // changement clavier
        assert!(!matches_shortcut("Désactivé", 75, false, true, false));
    }
    #[test]
    fn keyboard_sequences() {
        let mut keys = ShortcutKeys::default();
        let event =
            |keys: &mut ShortcutKeys, vk, down, shortcut| keys.event(shortcut, vk, down, true);
        assert_eq!(
            event(&mut keys, b'K' as u32, true, "Alt+K"),
            KeyDecision::default()
        );
        assert_eq!(
            event(&mut keys, b'K' as u32, false, "Alt+K"),
            KeyDecision::default()
        );
        event(&mut keys, 0xA4, true, "Alt+K");
        assert!(!keys.idle());
        assert_eq!(
            event(&mut keys, 75, true, "Alt+K"),
            KeyDecision {
                swallow: true,
                trigger: true
            }
        );
        assert_eq!(
            event(&mut keys, 75, true, "Alt+K"),
            KeyDecision {
                swallow: true,
                trigger: false
            }
        );
        assert_eq!(
            event(&mut keys, 75, false, "Alt+K"),
            KeyDecision {
                swallow: true,
                trigger: false
            }
        );
        event(&mut keys, 0xA4, false, "Alt+K");
        assert!(keys.idle());

        // Relâcher Alt avant K ne livre ni la répétition ni le key-up de K à Moonlight.
        event(&mut keys, 0xA5, true, "Alt+K");
        assert!(event(&mut keys, 75, true, "Alt+K").trigger);
        event(&mut keys, 0xA5, false, "Alt+K");
        assert!(!keys.idle());
        assert!(event(&mut keys, 75, false, "Alt+K").swallow);
        assert!(keys.idle());

        // AltGr (Ctrl droit + Alt droit), puis le réglage explicite avec Maj.
        event(&mut keys, 0xA3, true, "Alt+K");
        event(&mut keys, 0xA5, true, "Alt+K");
        assert_eq!(event(&mut keys, 75, true, "Alt+K"), KeyDecision::default());
        event(&mut keys, 75, false, "Alt+K");
        event(&mut keys, 0xA1, true, "Ctrl+Alt+Maj+K");
        assert!(event(&mut keys, 75, true, "Ctrl+Alt+Maj+K").trigger);
        assert!(event(&mut keys, 75, false, "Ctrl+Alt+Maj+K").swallow);
        event(&mut keys, 0xA1, false, "Ctrl+Alt+Maj+K");
        event(&mut keys, 0xA5, false, "Ctrl+Alt+Maj+K");
        event(&mut keys, 0xA3, false, "Ctrl+Alt+Maj+K");
        assert!(keys.idle());
        event(&mut keys, 0x12, true, "Alt+K");
        assert_eq!(keys.event("Alt+K", 75, true, false), KeyDecision::default());
        assert!(keys.idle());
    }
    #[test]
    fn quit_grace_period() {
        assert!(!quit_deadline_reached(Duration::ZERO));
        assert!(!quit_deadline_reached(Duration::from_millis(4999)));
        assert!(quit_deadline_reached(Duration::from_secs(5)));
        assert!(quit_deadline_reached(Duration::from_secs(6)));
    }
    #[test]
    fn formats_and_mode() {
        assert_eq!(duration(Duration::from_secs(3661)), "01:01:01");
        assert_eq!(latency(&[]), "indisponible");
        assert_eq!(latency(&[31, 7, 11, 25, 15]), "15 ms");
        let mut s = Settings::default();
        assert_eq!(display_mode(&s), "borderless");
        s.stream_menu_shortcut = "Désactivé".into();
        assert_eq!(display_mode(&s), "fullscreen");
        s.display_mode = "Fenêtré".into();
        assert_eq!(display_mode(&s), "windowed");
        assert!(profile(&s).contains("débit automatique"));
    }
}
