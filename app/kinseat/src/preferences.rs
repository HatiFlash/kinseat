//! Préférences propres à l'interface Hôte (distinctes des réglages du service).
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub reduced_motion: bool,
    pub default_profile: String,
    pub reserve_mb: u64,
    pub dismissed_logon_tasks: Vec<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            reduced_motion: false,
            default_profile: "Balanced".into(),
            reserve_mb: 2048,
            dismissed_logon_tasks: Vec::new(),
        }
    }
}

impl Preferences {
    fn validated(mut self) -> Self {
        if !matches!(
            self.default_profile.as_str(),
            "Economy" | "Balanced" | "Quality"
        ) {
            self.default_profile = "Balanced".into();
        }
        self.reserve_mb = self.reserve_mb.min(65536);
        self
    }
}

fn path() -> io::Result<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("LOCALAPPDATA absent"))?;
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .ok_or_else(|| io::Error::other("dossier utilisateur absent"))?;
    Ok(base.join("Kinseat").join("host-ui.json"))
}

fn read(path: &Path) -> Preferences {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Preferences>(&bytes).ok())
        .unwrap_or_default()
        .validated()
}

pub fn load() -> Preferences {
    path().map(|p| read(&p)).unwrap_or_default()
}

fn save(path: &Path, value: &Preferences) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("dossier absent"))?;
    fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec_pretty(&value.clone().validated())?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(".host-ui-{}-{nonce}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn store(value: &Preferences) -> io::Result<()> {
    save(&path()?, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_round_trip_and_replace() {
        let path = std::env::temp_dir().join(format!(
            "kinseat-prefs-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let value = Preferences {
            reduced_motion: true,
            default_profile: "Quality".into(),
            reserve_mb: 4096,
            dismissed_logon_tasks: vec!["\\HWiNFO".into()],
        };
        save(&path, &value).unwrap();
        assert_eq!(read(&path), value);
        save(&path, &Preferences::default()).unwrap();
        assert_eq!(read(&path), Preferences::default());
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn corrupt_and_invalid_values_fall_back() {
        let path =
            std::env::temp_dir().join(format!("kinseat-prefs-corrupt-{}.json", std::process::id()));
        fs::write(&path, b"{broken").unwrap();
        assert_eq!(read(&path), Preferences::default());
        fs::write(
            &path,
            br#"{"default_profile":"Unknown","reserve_mb":999999}"#,
        )
        .unwrap();
        assert_eq!(read(&path).default_profile, "Balanced");
        assert_eq!(read(&path).reserve_mb, 65536);
        fs::remove_file(path).unwrap();
    }
}
