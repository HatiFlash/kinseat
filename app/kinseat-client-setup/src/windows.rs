#![cfg(windows)]
use crate::{ENGINE_SHA256, ENGINE_ZIP, EXPECTED_SHA256, PAYLOAD};
use kinseat_client_setup::{Paths, UNINSTALL_KEY};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

pub fn paths() -> Result<Paths, String> {
    let var = |name| {
        env::var_os(name)
            .map(PathBuf::from)
            .ok_or_else(|| format!("{name} manquant"))
    };
    Ok(Paths::for_user(
        &var("LOCALAPPDATA")?,
        &var("APPDATA")?,
        &var("USERPROFILE")?.join("Desktop"),
    ))
}
fn log(p: &Paths, message: &str) {
    if let Some(parent) = p.log.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p.log)
    {
        let _ = writeln!(f, "{message}");
    }
}
/// Commande sans fenêtre console (le setup est une application fenêtrée).
fn hidden(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(program);
    cmd.creation_flags(0x0800_0000);
    cmd
}
fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let result = hidden(program)
        .args(args)
        .status()
        .map_err(|e| e.to_string())?;
    if result.success() {
        Ok(())
    } else {
        Err(format!("{program} : code {result}"))
    }
}
fn shortcut(target: &Path, link: &Path) -> Result<(), String> {
    fs::create_dir_all(link.parent().ok_or("Dossier de raccourci absent")?)
        .map_err(|e| e.to_string())?;
    let result = hidden("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", "$s=(New-Object -ComObject WScript.Shell).CreateShortcut($env:KINSEAT_LINK); $s.TargetPath=$env:KINSEAT_TARGET; $s.WorkingDirectory=Split-Path $env:KINSEAT_TARGET; $s.Save()"])
        .env("KINSEAT_TARGET", target).env("KINSEAT_LINK", link).status().map_err(|e| e.to_string())?;
    if result.success() {
        Ok(())
    } else {
        Err(format!("Raccourci : {result}"))
    }
}
fn register(p: &Paths) -> Result<(), String> {
    let uninstall = format!("\"{}\" --uninstall", p.setup.display());
    for (name, value) in [
        ("DisplayName", "Kinseat Client".to_owned()),
        ("DisplayVersion", env!("CARGO_PKG_VERSION").to_owned()),
        ("Publisher", "Kinseat".to_owned()),
        ("InstallLocation", p.program.display().to_string()),
        ("UninstallString", uninstall),
    ] {
        run(
            "reg.exe",
            &[
                "add",
                UNINSTALL_KEY,
                "/v",
                name,
                "/t",
                "REG_SZ",
                "/d",
                &value,
                "/f",
            ],
        )?;
    }
    Ok(())
}
/// Windows Firewall rule names; one per program, inbound, limited to the local network.
const FIREWALL_RULES: [(&str, &str); 2] = [
    ("Kinseat Client", "kinseat-client.exe"),
    ("Kinseat Client - diffusion", "moonlight\\Moonlight.exe"),
];

/// Without an allow rule Windows asks "Autoriser l'accès ?" the first time the client discovers
/// places or Moonlight receives the stream; that dialog steals the focus from the stream and a
/// child's "Annuler" creates a block rule. The per-user install cannot write rules itself, so
/// one consented elevation (UAC) adds them. Declining is not an error: the client still works with
/// a manual address, and Windows will ask again later.
pub fn allow_firewall(p: &Paths) -> Result<(), String> {
    let mut script = String::from("$ErrorActionPreference='Stop';");
    for (name, program) in FIREWALL_RULES {
        let program = p.program.join(program);
        script.push_str(&format!(
            "Get-NetFirewallRule -DisplayName '{name}' -EA SilentlyContinue | Remove-NetFirewallRule;\
             New-NetFirewallRule -DisplayName '{name}' -Direction Inbound -Action Allow -Profile Any \
             -RemoteAddress LocalSubnet -Program '{}' | Out-Null;",
            program.display().to_string().replace('\'', "''")
        ));
    }
    elevated(p, &script, "Autorisation du pare-feu")
}
fn remove_firewall(p: &Paths) {
    let present = hidden("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", "if (Get-NetFirewallRule -DisplayName 'Kinseat Client*' -EA SilentlyContinue) { exit 0 } else { exit 1 }"])
        .status()
        .is_ok_and(|s| s.success());
    if present {
        let _ = elevated(
            p,
            "Get-NetFirewallRule -DisplayName 'Kinseat Client*' -EA SilentlyContinue | Remove-NetFirewallRule",
            "Retrait des règles de pare-feu",
        );
    }
}
/// Runs a PowerShell script once with the administrator consent prompt; waits for it.
fn elevated(p: &Paths, script: &str, what: &str) -> Result<(), String> {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = kinseat_client_setup::base64(&utf16);
    let outer = format!(
        "$p = Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -Wait -PassThru \
         -ArgumentList '-NoProfile','-NonInteractive','-EncodedCommand','{encoded}'; exit $p.ExitCode"
    );
    let result = hidden("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &outer])
        .status()
        .map_err(|e| e.to_string())?;
    if result.success() {
        log(p, &format!("{what} : fait"));
        Ok(())
    } else {
        log(p, &format!("{what} : non fait ({result})"));
        Err(format!(
            "{what} non accordée : Windows demandera l'accès au premier lancement."
        ))
    }
}
pub fn install(p: &Paths, desktop: bool, menu: bool) -> Result<(), String> {
    let bytes = PAYLOAD.ok_or("Charge utile absente : reconstruire avec KINSEAT_CLIENT_EXE")?;
    if format!("{:x}", Sha256::digest(bytes)) != EXPECTED_SHA256 {
        return Err("SHA-256 de la charge utile invalide".into());
    }
    log(p, "Installation / mise à jour commencée");
    fs::create_dir_all(&p.program).map_err(|e| e.to_string())?;
    let staging = p.program.join("kinseat-client.exe.new");
    fs::write(&staging, bytes).map_err(|e| e.to_string())?;
    // Rename the old file aside before replacing it; restore it on failure.
    let backup = p.program.join("kinseat-client.exe.old");
    if p.client.exists() {
        let _ = fs::remove_file(&backup);
        if let Err(e) = fs::rename(&p.client, &backup) {
            let _ = fs::remove_file(&staging);
            return Err(format!(
                "Fermez Kinseat Client avant de mettre à jour : {e}"
            ));
        }
    }
    if let Err(e) = fs::rename(&staging, &p.client) {
        if backup.exists() {
            let _ = fs::rename(&backup, &p.client);
        }
        let _ = fs::remove_file(&staging);
        return Err(format!("Remplacement impossible : {e}"));
    }
    let _ = fs::remove_file(backup);
    install_engine(p)?;
    let self_exe = env::current_exe().map_err(|e| e.to_string())?;
    if self_exe != p.setup {
        fs::copy(self_exe, &p.setup).map_err(|e| e.to_string())?;
    }
    if desktop {
        shortcut(&p.client, &p.desktop_link)?;
    } else {
        let _ = fs::remove_file(&p.desktop_link);
    }
    if menu {
        shortcut(&p.client, &p.menu_link)?;
    } else {
        let _ = fs::remove_file(&p.menu_link);
    }
    register(p)?;
    log(p, "Installation terminée");
    Ok(())
}
/// Extrait le moteur Moonlight fourni (s'il est embarqué) dans `moonlight\`.
fn install_engine(p: &Paths) -> Result<(), String> {
    let Some(bytes) = ENGINE_ZIP else {
        log(p, "Aucun moteur de streaming embarqué");
        return Ok(());
    };
    if format!("{:x}", Sha256::digest(bytes)) != ENGINE_SHA256 {
        return Err("SHA-256 du moteur de streaming invalide".into());
    }
    let target = p.program.join("moonlight");
    let staging = p.program.join("moonlight.new");
    let _ = fs::remove_dir_all(&staging);
    kinseat_client_setup::extract_zip(bytes, &staging)?;
    // Moonlight is GPLv3 and redistributed unmodified: ship its licence and the source notice with it.
    for (name, text) in [
        (
            "GPL-3.0.txt",
            include_str!("../../../installer/licenses/GPL-3.0.txt"),
        ),
        (
            "THIRD-PARTY-NOTICES.txt",
            include_str!("../../../installer/licenses/THIRD-PARTY-NOTICES.txt"),
        ),
    ] {
        fs::write(staging.join(name), text).map_err(|e| format!("{name} : {e}"))?;
    }
    let old = p.program.join("moonlight.old");
    let _ = fs::remove_dir_all(&old);
    if target.exists() {
        fs::rename(&target, &old)
            .map_err(|e| format!("Fermez Kinseat Client avant de mettre à jour : {e}"))?;
    }
    if let Err(e) = fs::rename(&staging, &target) {
        if old.exists() {
            let _ = fs::rename(&old, &target);
        }
        return Err(format!("Installation du moteur impossible : {e}"));
    }
    let _ = fs::remove_dir_all(old);
    log(p, "Moteur de streaming installé");
    Ok(())
}
pub fn uninstall(p: &Paths, erase_settings: bool) -> Result<(), String> {
    log(p, "Désinstallation commencée");
    let engine = p.program.join("moonlight");
    if engine.exists() {
        fs::remove_dir_all(&engine).map_err(|e| format!("{} : {e}", engine.display()))?;
    }
    for file in p.uninstall_files(erase_settings) {
        if file == p.setup && env::current_exe().ok().as_deref() == Some(file) {
            continue;
        }
        if let Err(e) = fs::remove_file(file) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(format!("{} : {e}", file.display()));
            }
        }
    }
    // "Supprimer aussi les réglages personnels": the whole client data folder — settings, the
    // engine's pairing identity (its private key) and the logs — not only client.json.
    if erase_settings {
        if let Some(data) = p.settings.parent().filter(|d| d.exists()) {
            fs::remove_dir_all(data).map_err(|e| format!("{} : {e}", data.display()))?;
        }
    }
    remove_firewall(p);
    run("reg.exe", &["delete", UNINSTALL_KEY, "/f"])?;
    log(p, "Désinstallation terminée");
    schedule_self_removal(p);
    Ok(())
}

/// A running executable cannot delete itself: when the uninstaller is the installed `setup.exe`,
/// hand its removal (and the then-empty program folder) to a detached `cmd` that waits for this
/// process to exit. Logs and settings stay untouched.
fn schedule_self_removal(p: &Paths) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    if env::current_exe().ok().as_deref() != Some(p.setup.as_path()) {
        let _ = fs::remove_dir(&p.program);
        return;
    }
    let script = format!(
        "ping -n 3 127.0.0.1 >nul & del /f /q \"{}\" & rmdir \"{}\"",
        p.setup.display(),
        p.program.display()
    );
    let _ = std::process::Command::new("cmd.exe")
        .arg("/c")
        .raw_arg(script)
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn();
}
