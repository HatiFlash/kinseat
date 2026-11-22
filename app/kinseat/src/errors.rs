use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Stopped,
    Permission,
    Busy,
    Other,
}

pub fn classify(error: &io::Error) -> Kind {
    if kinseat_ipc::is_forbidden(error) {
        return Kind::Permission;
    }
    match error.raw_os_error() {
        Some(2) => Kind::Stopped,
        Some(5) => Kind::Permission,
        Some(231) => Kind::Busy,
        _ => Kind::Other,
    }
}

pub fn description(kind: Kind) -> (&'static str, &'static str) {
    match kind {
        Kind::Stopped => ("Service arrêté", "Le service Kinseat ne répond pas. Vérifiez qu’il est démarré dans les Services Windows."),
        Kind::Permission => ("Autorisation nécessaire", "Kinseat doit être autorisé une seule fois sur ce compte Windows (droits administrateur). Ensuite, l’application s’ouvre sans nouvelle demande."),
        Kind::Busy => ("Service occupé", "La connexion est momentanément indisponible. Un nouvel essai sera effectué automatiquement."),
        Kind::Other => ("Connexion impossible", "Vérifiez le service Kinseat, puis réessayez. Si le problème persiste, consultez les détails."),
    }
}

/// Processus lancé avec les droits administrateur (jeton élevé).
#[cfg(windows)]
pub fn is_elevated() -> bool {
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

/// Relance l'application en administrateur avec `--authorize` (UAC unique pour ce compte).
#[cfg(windows)]
pub fn elevate() -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let exe = std::env::current_exe()?;
    let path: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
    let verb: Vec<u16> = "runas".encode_utf16().chain(Some(0)).collect();
    let args: Vec<u16> = "--authorize".encode_utf16().chain(Some(0)).collect();
    // ShellExecuteW returns a value <= 32 on failure, including a cancelled UAC prompt.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            path.as_ptr(),
            args.as_ptr(),
            std::ptr::null(),
            1,
        )
    };
    if result as isize <= 32 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// French label of a `seat.delete` report item (contract keys stay in English on the wire).
pub fn delete_item_label(item: &str) -> String {
    match item {
        "seat" => "place du joueur",
        "shortcuts" => "raccourcis des jeux",
        "preset" => "réglages de la place",
        "firewall" => "règles du pare-feu",
        "library" => "droits de la bibliothèque",
        "pairings" => "appareils appairés",
        "session" => "session Windows",
        "apollo" => "données de diffusion",
        "profile" => "profil et fichiers du joueur",
        "account" => "compte Windows",
        "registry" => "inscription du compte",
        other => other,
    }
    .to_owned()
}

pub fn removed_summary(removed: &[String]) -> String {
    if removed.is_empty() {
        return "Rien n'a été supprimé.".to_owned();
    }
    let items: Vec<String> = removed.iter().map(|i| delete_item_label(i)).collect();
    format!("Supprimé : {}.", items.join(", "))
}

/// Why the Windows account was kept (`accountKept` code from the service).
pub fn account_kept_label(code: &str) -> String {
    match code {
        "not_managed" => "ce compte n'a pas été créé par Kinseat ou est protégé".to_owned(),
        "account_unknown" | "identity_unknown" => "compte introuvable ou non vérifiable".to_owned(),
        "caller_unknown" | "console_unknown" | "groups_unknown" => {
            "vérification de sécurité impossible, compte laissé en place".to_owned()
        }
        other => other.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn delete_report_is_in_french() {
        let removed = vec!["profile".to_owned(), "account".to_owned()];
        assert_eq!(
            super::removed_summary(&removed),
            "Supprimé : profil et fichiers du joueur, compte Windows."
        );
        assert_eq!(super::removed_summary(&[]), "Rien n'a été supprimé.");
        assert!(super::account_kept_label("not_managed").contains("Kinseat"));
        assert_eq!(super::delete_item_label("seat"), "place du joueur");
        assert_eq!(super::delete_item_label("shortcuts"), "raccourcis des jeux");
        assert_eq!(super::delete_item_label("inconnu"), "inconnu");
    }

    use super::*;
    #[test]
    fn errors_are_actionable() {
        assert_eq!(classify(&io::Error::from_raw_os_error(2)), Kind::Stopped);
        assert_eq!(classify(&io::Error::from_raw_os_error(5)), Kind::Permission);
        assert_eq!(classify(&io::Error::from_raw_os_error(231)), Kind::Busy);
        assert_eq!(classify(&io::Error::from_raw_os_error(123)), Kind::Other);
        assert_eq!(classify(&io::Error::other("réseau")), Kind::Other);
    }
}
