//! Journal local sans données de session ni messages saisis par les utilisateurs.
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};
use std::time::{SystemTime, UNIX_EPOCH};

const LIMIT: u64 = 5 * 1024 * 1024;
static LOGGER: OnceLock<Mutex<PathBuf>> = OnceLock::new();
static DETAILED: AtomicBool = AtomicBool::new(false);

pub fn set_detailed(value: bool) {
    DETAILED.store(value, Ordering::Relaxed);
}

fn path() -> io::Result<PathBuf> {
    #[cfg(windows)]
    let dir = PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or_else(|| io::Error::other("LOCALAPPDATA absent"))?,
    )
    .join("Kinseat")
    .join("logs");
    #[cfg(not(windows))]
    let dir =
        if let Some(state) = std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
            PathBuf::from(state)
        } else {
            PathBuf::from(std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME absent"))?)
                .join(".local/state")
        }
        .join("kinseat");
    fs::create_dir_all(&dir)?;
    Ok(dir.join("kinseat-app.log"))
}

pub fn init() {
    match path() {
        Ok(path) => {
            let _ = LOGGER.set(Mutex::new(path));
        }
        Err(error) => eprintln!("Journal local indisponible : {error}"),
    }
    std::panic::set_hook(Box::new(|info| {
        // Ne jamais écrire le payload de la panique : il peut contenir une donnée privée.
        let location = info
            .location()
            .map(|loc| format!("{}:{}", loc.file(), loc.line()))
            .unwrap_or_else(|| "emplacement inconnu".into());
        record("ERROR", &format!("Panique à {location}"));
    }));
}

pub fn record(level: &str, message: &str) {
    if level == "DEBUG" && !DETAILED.load(Ordering::Relaxed) {
        return;
    }
    let Some(lock) = LOGGER.get() else { return };
    let Ok(path) = lock.lock() else { return };
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let line = format!("{} [{level}] {message}\n", utc(seconds));
    if let Err(error) = write(&path, line.as_bytes()) {
        eprintln!("Écriture du journal local impossible : {error}");
    }
}

fn write(path: &PathBuf, bytes: &[u8]) -> io::Result<()> {
    let len = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if len.saturating_add(bytes.len() as u64) > LIMIT {
        let old = path.with_extension("log.1");
        match fs::remove_file(&old) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        if len > 0 {
            fs::rename(path, old)?;
        }
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(bytes)
}

// Conversion grégorienne UTC, indépendante du fuseau du poste (jours depuis l'époque Unix).
fn utc(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let seconds = seconds % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utc_dates() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(1_700_000_000), "2023-11-14T22:13:20Z");
    }
    #[test]
    fn rotates_at_five_mib() {
        let dir = std::env::temp_dir().join(format!(
            "kinseat-log-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("kinseat-app.log");
        let file = fs::File::create(&path).unwrap();
        file.set_len(LIMIT).unwrap();
        write(&path, b"new\n").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new\n");
        assert_eq!(
            fs::metadata(path.with_extension("log.1")).unwrap().len(),
            LIMIT
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
