//! Profil du joueur : pseudo, code PIN local (hash salé) et identifiant d'appareil stable.
use crate::settings::Settings;
use rand::RngCore;
use sha2::{Digest, Sha256};

pub const PSEUDO_MIN: usize = 2;
pub const PSEUDO_MAX: usize = 24;
pub const PIN_MIN: usize = 4;
pub const PIN_MAX: usize = 8;

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, b| {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// Pseudo : 2 à 24 caractères, lettres, chiffres, espace, `-`, `_`, `.`. Renvoie la forme nettoyée.
pub fn validate_pseudo(raw: &str) -> Result<String, &'static str> {
    let pseudo = raw.trim();
    let count = pseudo.chars().count();
    if count < PSEUDO_MIN {
        return Err("Le pseudo doit contenir au moins 2 caractères.");
    }
    if count > PSEUDO_MAX {
        return Err("Le pseudo est limité à 24 caractères.");
    }
    if !pseudo
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
    {
        return Err("Le pseudo accepte lettres, chiffres, espace, - _ et . uniquement.");
    }
    Ok(pseudo.to_owned())
}

/// Code PIN : 4 à 8 chiffres.
pub fn validate_pin(pin: &str) -> Result<(), &'static str> {
    if (PIN_MIN..=PIN_MAX).contains(&pin.len()) && pin.chars().all(|c| c.is_ascii_digit()) {
        Ok(())
    } else {
        Err("Le code PIN doit contenir de 4 à 8 chiffres.")
    }
}

/// sha256(sel ‖ pin), en hexadécimal.
pub fn hash_pin(salt: &[u8], pin: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(pin.as_bytes());
    to_hex(&hasher.finalize())
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Génère (sel hex, hash hex) pour un PIN, avec un sel aléatoire de 16 octets.
pub fn make_pin(pin: &str) -> (String, String) {
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    (to_hex(&salt), hash_pin(&salt, pin))
}

pub fn verify_pin(salt_hex: &str, hash_hex: &str, pin: &str) -> bool {
    let Some(salt) = from_hex(salt_hex) else {
        return false;
    };
    let candidate = hash_pin(&salt, pin);
    // Comparaison sans sortie anticipée.
    candidate.len() == hash_hex.len()
        && candidate
            .bytes()
            .zip(hash_hex.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

pub fn has_pin(s: &Settings) -> bool {
    !s.pin_hash.is_empty() && !s.pin_salt.is_empty()
}

pub fn check_pin(s: &Settings, pin: &str) -> bool {
    has_pin(s) && verify_pin(&s.pin_salt, &s.pin_hash, pin)
}

/// Identifiant d'appareil stable (32 hexadécimaux), généré une seule fois.
pub fn ensure_device_id(s: &mut Settings) -> bool {
    let valid = (8..=64).contains(&s.device_id.len())
        && s.device_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-');
    if valid {
        return false;
    }
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    s.device_id = to_hex(&bytes);
    true
}

/// Nom de l'appareil envoyé à l'Hôte : réglage manuel, sinon nom du PC Windows.
pub fn device_name(s: &Settings) -> String {
    let raw = [
        s.device_name.clone(),
        std::env::var("COMPUTERNAME").unwrap_or_default(),
        std::env::var("HOSTNAME").unwrap_or_default(),
    ]
    .into_iter()
    .map(|v| v.trim().to_owned())
    .find(|v| !v.is_empty())
    .unwrap_or_else(|| "Client Kinseat".into());
    let cleaned: String = raw.chars().filter(|c| !c.is_control()).take(40).collect();
    if cleaned.trim().is_empty() {
        "Client Kinseat".into()
    } else {
        cleaned
    }
}

fn set_pin(s: &mut Settings, pin: &str, confirm: &str) -> Result<(), String> {
    validate_pin(pin).map_err(str::to_owned)?;
    if pin != confirm {
        return Err("La confirmation du code PIN ne correspond pas.".into());
    }
    let (salt, hash) = make_pin(pin);
    s.pin_salt = salt;
    s.pin_hash = hash;
    Ok(())
}

/// Premier lancement : pseudo obligatoire, PIN facultatif (vide = aucun).
pub fn create_profile(
    s: &mut Settings,
    pseudo: &str,
    pin: &str,
    confirm: &str,
) -> Result<(), String> {
    let pseudo = validate_pseudo(pseudo).map_err(str::to_owned)?;
    let mut next = s.clone();
    if !pin.is_empty() || !confirm.is_empty() {
        set_pin(&mut next, pin, confirm)?;
    }
    next.pseudo = pseudo;
    ensure_device_id(&mut next);
    *s = next;
    Ok(())
}

pub fn change_pseudo(s: &mut Settings, pseudo: &str) -> Result<(), String> {
    s.pseudo = validate_pseudo(pseudo).map_err(str::to_owned)?;
    Ok(())
}

/// Définit ou remplace le PIN ; si un PIN existe, le code actuel est exigé.
pub fn change_pin(s: &mut Settings, current: &str, pin: &str, confirm: &str) -> Result<(), String> {
    if has_pin(s) && !check_pin(s, current) {
        return Err("Le code PIN actuel est incorrect.".into());
    }
    set_pin(s, pin, confirm)
}

pub fn remove_pin(s: &mut Settings, current: &str) -> Result<(), String> {
    if has_pin(s) && !check_pin(s, current) {
        return Err("Le code PIN actuel est incorrect.".into());
    }
    s.pin_salt.clear();
    s.pin_hash.clear();
    Ok(())
}

/// Champs de saisie des écrans de profil.
#[derive(Default)]
pub struct Form {
    pub pseudo: String,
    pub current: String,
    pub pin: String,
    pub confirm: String,
    /// (succès, message)
    pub message: Option<(bool, String)>,
}
impl Form {
    pub fn clear_pins(&mut self) {
        self.current.clear();
        self.pin.clear();
        self.confirm.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pseudo_rules() {
        assert_eq!(validate_pseudo("  Yanis ").unwrap(), "Yanis");
        assert!(validate_pseudo("Lina_2.0 - x").is_ok());
        assert!(validate_pseudo("é").is_err());
        assert!(validate_pseudo("a").is_err());
        assert!(validate_pseudo(&"a".repeat(25)).is_err());
        assert!(validate_pseudo(&"a".repeat(24)).is_ok());
        assert!(validate_pseudo("Bob<script>").is_err());
        assert!(validate_pseudo("   ").is_err());
    }

    #[test]
    fn pin_rules() {
        for ok in ["1234", "12345678"] {
            assert!(validate_pin(ok).is_ok(), "{ok}");
        }
        for bad in ["", "123", "123456789", "12a4", "12 4", "１２３４"] {
            assert!(validate_pin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn hash_is_salted_sha256() {
        // sha256("" ‖ "abc") connu.
        assert_eq!(
            hash_pin(b"", "abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let (s1, h1) = make_pin("1234");
        let (s2, h2) = make_pin("1234");
        assert_eq!(s1.len(), 32);
        assert_ne!(s1, s2);
        assert_ne!(h1, h2);
        assert!(verify_pin(&s1, &h1, "1234"));
        assert!(!verify_pin(&s1, &h1, "1235"));
        assert!(!verify_pin(&s1, &h2, "1234"));
        assert!(!verify_pin("zz", &h1, "1234"));
        assert!(!h1.contains("1234"));
    }

    #[test]
    fn profile_lifecycle() {
        let mut s = Settings::default();
        assert!(create_profile(&mut s, "x", "", "").is_err());
        assert!(s.pseudo.is_empty() && s.device_id.is_empty());
        assert!(create_profile(&mut s, "Yanis", "1234", "9999").is_err());
        assert!(s.pseudo.is_empty() && !has_pin(&s));
        create_profile(&mut s, "Yanis", "", "").unwrap();
        assert_eq!(s.pseudo, "Yanis");
        assert!(!has_pin(&s));
        let id = s.device_id.clone();
        assert_eq!(id.len(), 32);
        assert!(!ensure_device_id(&mut s));
        assert_eq!(s.device_id, id);

        change_pin(&mut s, "", "4321", "4321").unwrap();
        assert!(check_pin(&s, "4321") && !check_pin(&s, "1234"));
        assert!(change_pin(&mut s, "0000", "1111", "1111").is_err());
        change_pin(&mut s, "4321", "1111", "1111").unwrap();
        assert!(check_pin(&s, "1111"));
        assert!(remove_pin(&mut s, "4321").is_err());
        assert!(has_pin(&s));
        remove_pin(&mut s, "1111").unwrap();
        assert!(!has_pin(&s));
        assert!(!check_pin(&s, ""));
        change_pseudo(&mut s, "Lina").unwrap();
        assert!(change_pseudo(&mut s, "!").is_err());
        assert_eq!(s.pseudo, "Lina");
    }

    #[test]
    fn device_name_is_bounded() {
        let mut s = Settings {
            device_name: "x".repeat(80),
            ..Settings::default()
        };
        assert_eq!(device_name(&s).chars().count(), 40);
        s.device_name = " PC-Yanis ".into();
        assert_eq!(device_name(&s), "PC-Yanis");
    }
}
