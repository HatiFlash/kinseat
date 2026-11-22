use crate::plan::{verify_manifest, Manifest, Plan};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Deserialize)]
pub struct Progress {
    pub step: String,
    pub status: String,
    pub message: String,
    pub pct: f32,
}

pub fn progress_lines(path: &Path) -> Vec<Progress> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/host_payload.rs"));
}

/// Dossier `payload` : à côté de l'exe (paquet de développement) ou, pour le setup livré en un
/// seul .exe, la charge utile embarquée extraite dans un dossier neuf. Le processus élevé
/// l'extrait dans un dossier qu'il crée sous %SystemRoot%\Temp : créé par un administrateur,
/// il n'est pas modifiable par le jeton non élevé du même utilisateur entre la vérification
/// et l'exécution des scripts.
pub fn payload_root() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let side = exe.parent().ok_or("Dossier setup absent")?.join("payload");
    if side.join("manifest.json").is_file() {
        return Ok(side);
    }
    let Some(bytes) = embedded::HOST_PAYLOAD else {
        return Ok(side);
    };
    // Extraction neuve à chaque appel : l'appelant la supprime avec cleanup_payload.
    extract_embedded(bytes, embedded::HOST_PAYLOAD_SHA256, &exe)
}

pub fn is_embedded_root(root: &Path) -> bool {
    embedded::HOST_PAYLOAD.is_some()
        && root
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("kinseat-setup-"))
}

/// Supprime l'extraction temporaire (sans effet pour un paquet à dossier `payload`).
pub fn cleanup_payload(root: &Path) {
    if is_embedded_root(root) {
        if let Some(dir) = root.parent() {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

fn extract_embedded(bytes: &[u8], expected: &str, exe: &Path) -> Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    if format!("{:x}", Sha256::digest(bytes)) != expected {
        return Err("Charge utile embarquée corrompue (SHA-256)".into());
    }
    let base = std::env::var_os("SystemRoot")
        .map(|r| PathBuf::from(r).join("Temp"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let dir = base.join(format!("kinseat-setup-{}-{stamp}", std::process::id()));
    fs::create_dir(&dir).map_err(|e| format!("Extraction impossible : {e}"))?;
    let root = dir.join("payload");
    let result = (|| -> Result<(), String> {
        unpack(bytes, &root)?;
        // Install-Kinseat.ps1 recopie <parent du payload>\setup.exe dans le dossier installé
        // (désinstallation / réparation depuis « Applications installées »).
        fs::copy(exe, dir.join("setup.exe")).map_err(|e| e.to_string())?;
        Ok(())
    })();
    match result {
        Ok(()) => Ok(root),
        Err(e) => {
            let _ = fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

/// Extraction stricte : séparateurs `\` ou `/` acceptés, aucun chemin absolu ni `..`.
fn unpack(bytes: &[u8], root: &Path) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().replace('\\', "/");
        let mut target = root.to_path_buf();
        for part in name.split('/').filter(|p| !p.is_empty()) {
            if part == "." || part == ".." || part.contains(':') {
                return Err(format!("Chemin refusé dans la charge utile : {name}"));
            }
            target.push(part);
        }
        if target == root {
            continue;
        }
        if name.ends_with('/') {
            fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut file = fs::File::create(&target).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut file).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn verify(root: &Path) -> Result<Manifest, String> {
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let required = Manifest {
        version: manifest.version.clone(),
        artifacts: manifest
            .artifacts
            .iter()
            .filter(|a| a.required)
            .cloned()
            .collect(),
    };
    if required.artifacts.is_empty() {
        return Err("Aucun fichier obligatoire".into());
    }
    verify_manifest(&required, root)?;
    for name in ["Install-Kinseat.ps1", "Uninstall-Kinseat.ps1"] {
        if !required.artifacts.iter().any(|a| a.file == name) {
            return Err(format!("Script non authentifié : {name}"));
        }
    }
    Ok(manifest)
}

pub fn verify_optionals(manifest: &Manifest, root: &Path, plan: &Plan) -> Vec<String> {
    let selected = [
        ("apollo", plan.options.apollo),
        ("vdd", plan.options.virtual_display),
    ];
    selected
        .into_iter()
        .filter(|(_, on)| *on)
        .filter_map(
            |(id, _)| match manifest.artifacts.iter().find(|a| a.id == id) {
                Some(a)
                    if verify_manifest(
                        &Manifest {
                            version: manifest.version.clone(),
                            artifacts: vec![a.clone()],
                        },
                        root,
                    )
                    .is_ok() =>
                {
                    None
                }
                _ => Some(format!("{id} : ignoré (absent ou empreinte non qualifiée)")),
            },
        )
        .collect()
}

pub fn run_apply(
    plan_file: &Path,
    uninstall: bool,
    progress: &Path,
    log: &Path,
) -> Result<(), String> {
    let root = payload_root()?;
    let manifest = verify(&root)?;
    let plan: Plan = serde_json::from_slice(&fs::read(plan_file).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let canonical = crate::plan::plan(plan.options.clone())?;
    if plan.steps != canonical.steps {
        return Err("Étapes du plan modifiées après confirmation".into());
    }
    if !uninstall {
        let _ = verify_optionals(&manifest, &root, &plan);
    }
    let script = root.join(if uninstall {
        "Uninstall-Kinseat.ps1"
    } else {
        "Install-Kinseat.ps1"
    });
    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-WindowStyle",
        "Hidden",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ])
    .arg(script);
    #[cfg(windows)]
    {
        // Aucune console visible : le setup est une application fenêtrée.
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    if is_embedded_root(&root) {
        // Le setup.exe recopié contient déjà la charge utile : pas de dossier payload installé.
        cmd.env("KINSEAT_PAYLOAD_EMBEDDED", "1");
    }
    if uninstall {
        cmd.arg("-KeepData");
    } else {
        cmd.arg("-Plan").arg(plan_file);
    }
    let status = cmd
        .arg("-Log")
        .arg(log)
        .arg("-Progress")
        .arg(progress)
        .status()
        .map_err(|e| e.to_string());
    cleanup_payload(&root);
    let status = status?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "PowerShell : code {:?}; journal {}",
            status.code(),
            log.display()
        ))
    }
}

#[cfg(windows)]
pub fn elevate(plan: &Path, progress: &Path, log: &Path, uninstall: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, WaitForSingleObject, INFINITE,
    };
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    fn wide(v: &std::ffi::OsStr) -> Vec<u16> {
        v.encode_wide().chain(Some(0)).collect()
    }
    let exe = wide(
        std::env::current_exe()
            .map_err(|e| e.to_string())?
            .as_os_str(),
    );
    let verb = wide(std::ffi::OsStr::new("runas"));
    let params = format!(
        "--apply \"{}\" --progress \"{}\" --log \"{}\" {}",
        plan.display(),
        progress.display(),
        log.display(),
        if uninstall { "--uninstall" } else { "" }
    );
    let args = wide(std::ffi::OsStr::new(&params));
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = exe.as_ptr();
    info.lpParameters = args.as_ptr();
    info.nShow = 0; // SW_HIDE : le processus élevé travaille sans fenêtre.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(format!(
            "Élévation refusée : {}",
            std::io::Error::last_os_error()
        ));
    }
    if info.hProcess.is_null() {
        return Err("Processus élevé introuvable".into());
    }
    let wait = unsafe { WaitForSingleObject(info.hProcess, INFINITE) };
    let mut code = 0;
    let read = unsafe { GetExitCodeProcess(info.hProcess, &mut code) };
    unsafe { CloseHandle(info.hProcess) };
    if wait != 0 || read == 0 || code != 0 {
        return Err(format!("Installateur élevé : code {code}, attente {wait}"));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn elevate(_: &Path, _: &Path, _: &Path, _: bool) -> Result<(), String> {
    Err("Installation Windows uniquement".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            for (name, data) in entries {
                w.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                std::io::Write::write_all(&mut w, data).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn unpack_accepts_backslashes_and_refuses_escapes() {
        let dir = std::env::temp_dir().join(format!("ks-unpack-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let ok = zip_of(&[("service\\a.txt", b"A"), ("manifest.json", b"{}")]);
        unpack(&ok, &dir).unwrap();
        assert_eq!(fs::read(dir.join("service").join("a.txt")).unwrap(), b"A");
        for bad in ["../x.txt", "a/../../x.txt", "C:/x.txt"] {
            assert!(
                unpack(&zip_of(&[(bad, b"X")]), &dir.join("b")).is_err(),
                "{bad}"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// PowerShell traite ‘ ’ ‚ ‛ comme des apostrophes : dans une chaîne entre apostrophes, une
    /// apostrophe typographique ferme la chaîne et casse tout le script (désinstallation 0.3.2/0.3.3).
    #[test]
    fn payload_scripts_have_no_typographic_quotes() {
        for (name, text) in [
            (
                "Install-Kinseat.ps1",
                include_str!("../../../installer/payload/Install-Kinseat.ps1"),
            ),
            (
                "Uninstall-Kinseat.ps1",
                include_str!("../../../installer/payload/Uninstall-Kinseat.ps1"),
            ),
        ] {
            for (line, content) in text.lines().enumerate() {
                assert!(
                    !content.contains(['\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}']),
                    "{name}:{} contient une apostrophe typographique",
                    line + 1
                );
            }
        }
    }

    #[test]
    fn corrupted_payload_is_refused() {
        let exe = std::env::current_exe().unwrap();
        assert!(extract_embedded(b"abc", "00", &exe).is_err());
    }
    #[test]
    fn powershell_contract() {
        let install = include_str!("../../../installer/payload/Install-Kinseat.ps1");
        let uninstall = include_str!("../../../installer/payload/Uninstall-Kinseat.ps1");
        for (script, params) in [
            (install, &["$Plan", "$Log", "$Progress"][..]),
            (uninstall, &["$KeepData", "$Log", "$Progress"][..]),
        ] {
            for param in params {
                assert!(script.contains(param), "paramètre absent : {param}");
            }
            for field in ["step", "status", "message", "pct"] {
                assert!(script.contains(field), "champ JSONL absent : {field}");
            }
        }
        for field in [
            "options",
            "install_dir",
            "rdpwrap",
            "rdp_consent",
            "keep_network_rdp",
            "defender_exclusion",
            "apollo",
            "virtual_display",
        ] {
            assert!(install.contains(field), "champ plan absent : {field}");
        }
    }
    #[test]
    fn jsonl_ignores_incomplete_tail() {
        let dir = std::env::temp_dir().join(format!("kinseat-jsonl-{}", std::process::id()));
        fs::write(
            &dir,
            "{\"step\":\"a\",\"status\":\"ok\",\"message\":\"fait\",\"pct\":50}\n{\"step\":",
        )
        .unwrap();
        assert_eq!(progress_lines(&dir).len(), 1);
        fs::remove_file(dir).unwrap();
    }
}
