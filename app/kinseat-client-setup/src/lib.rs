use std::path::{Path, PathBuf};

pub const UNINSTALL_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\KinseatClient";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub program: PathBuf,
    pub client: PathBuf,
    pub setup: PathBuf,
    pub log: PathBuf,
    pub settings: PathBuf,
    pub desktop_link: PathBuf,
    pub menu_link: PathBuf,
}

impl Paths {
    pub fn for_user(local: &Path, roaming: &Path, desktop: &Path) -> Self {
        let program = local.join("Programs").join("Kinseat Client");
        Self {
            client: program.join("kinseat-client.exe"),
            setup: program.join("setup.exe"),
            log: local.join("Kinseat Client").join("logs").join("setup.log"),
            settings: roaming.join("kinseat").join("client.json"),
            desktop_link: desktop.join("Kinseat Client.lnk"),
            menu_link: roaming
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Kinseat Client.lnk"),
            program,
        }
    }

    pub fn uninstall_files(&self, erase_settings: bool) -> Vec<&Path> {
        let mut files = vec![
            self.client.as_path(),
            self.setup.as_path(),
            self.desktop_link.as_path(),
            self.menu_link.as_path(),
        ];
        if erase_settings {
            files.push(self.settings.as_path());
        }
        files
    }
}

/// Extrait une archive ZIP dans `dest`. Si toutes les entrées partagent un même dossier
/// racine (archive « Moonlight-x.y/… »), ce dossier est retiré. Refuse les chemins sortants.
pub fn extract_zip(bytes: &[u8], dest: &Path) -> Result<usize, String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let names: Vec<PathBuf> = (0..archive.len())
        .map(|i| {
            archive
                .by_index(i)
                .map_err(|e| e.to_string())?
                .enclosed_name()
                .ok_or_else(|| "Chemin d'archive refusé".to_string())
        })
        .collect::<Result<_, _>>()?;
    let root = names.first().and_then(|n| n.components().next());
    let strip = root.is_some()
        && names.iter().all(|n| n.components().next() == root)
        && names.iter().any(|n| n.components().count() > 1);
    let mut files = 0;
    for (i, name) in names.iter().enumerate() {
        let rel: PathBuf = if strip {
            name.components().skip(1).collect()
        } else {
            name.clone()
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(&rel);
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut f = std::fs::File::create(&out).map_err(|e| format!("{} : {e}", out.display()))?;
        std::io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
        files += 1;
    }
    Ok(files)
}

/// Standard base64 (PowerShell `-EncodedCommand` takes UTF-16LE bytes in this form).
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foobar"), "Zm9vYmFy");
    }
    use super::*;
    #[test]
    fn paths_stay_per_user() {
        let p = Paths::for_user(
            Path::new("/user/local"),
            Path::new("/user/roaming"),
            Path::new("/user/desktop"),
        );
        assert_eq!(
            p.client,
            Path::new("/user/local/Programs/Kinseat Client/kinseat-client.exe")
        );
        assert_eq!(
            p.menu_link,
            Path::new("/user/roaming/Microsoft/Windows/Start Menu/Programs/Kinseat Client.lnk")
        );
        assert!(!p.uninstall_files(false).contains(&p.settings.as_path()));
        assert!(p.uninstall_files(true).contains(&p.settings.as_path()));
        assert_eq!(p.uninstall_files(false).len(), 4);
    }

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut w = zip::ZipWriter::new(&mut buf);
        for (name, data) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
        buf.into_inner()
    }

    #[test]
    fn extract_strips_common_root() {
        let dir = std::env::temp_dir().join(format!("ks-zip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let z = zip_of(&[
            ("Moonlight-6/Moonlight.exe", b"exe"),
            ("Moonlight-6/Qt/a.dll", b"d"),
        ]);
        assert_eq!(extract_zip(&z, &dir).unwrap(), 2);
        assert!(dir.join("Moonlight.exe").exists());
        assert!(dir.join("Qt/a.dll").exists());
        let _ = std::fs::remove_dir_all(&dir);
        let z = zip_of(&[("Moonlight.exe", b"exe"), ("Qt/a.dll", b"d")]);
        assert_eq!(extract_zip(&z, &dir).unwrap(), 2);
        assert!(dir.join("Moonlight.exe").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_refuses_escape() {
        let dir = std::env::temp_dir().join(format!("ks-zip-esc-{}", std::process::id()));
        let z = zip_of(&[("../evil.txt", b"x")]);
        assert!(extract_zip(&z, &dir).is_err());
    }
}
