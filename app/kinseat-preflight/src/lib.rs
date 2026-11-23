//! Inventaire sans effet : les sondes lisent, l'évaluation est pure et testable.
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Statut {
    Ok,
    Attention,
    Bloquant,
}

#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub titre: &'static str,
    pub statut: Statut,
    pub valeur_trouvee: String,
    pub attendu: &'static str,
    pub conseil: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct Service {
    pub present: bool,
    pub actif: bool,
}
#[derive(Clone, Debug, Default)]
pub struct Port {
    pub numero: u16,
    pub protocole: String,
    pub processus: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct SystemFacts {
    pub windows_major: Option<u32>,
    pub build: Option<u32>,
    pub edition: Option<String>,
    pub admin: Option<bool>,
    pub term_service: Service,
    pub vigem_bus: Service,
    pub hid_hide: Service,
    pub apollo: Service,
    pub multiseat: Service,
    pub ports: Vec<Port>,
    pub ports_known: bool,
    pub disque_go: Option<u64>,
    pub gpu: Option<String>,
    pub reboot_pending: Option<bool>,
    pub rdpwrap: Option<bool>,
}

pub trait Probe {
    fn collect(&self) -> SystemFacts;
}
pub struct LocalProbe;
#[cfg(not(windows))]
impl Probe for LocalProbe {
    fn collect(&self) -> SystemFacts {
        SystemFacts::default()
    }
}
#[cfg(windows)]
mod windows;
#[cfg(windows)]
impl Probe for LocalProbe {
    fn collect(&self) -> SystemFacts {
        windows::collect()
    }
}

fn yes_no(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "oui",
        Some(false) => "non",
        None => "inconnu",
    }
}
fn service_label(svc: &Service, installed_by_setup: bool) -> &'static str {
    match (svc.present, svc.actif) {
        (true, true) => "installé et démarré",
        (true, false) => "installé, arrêté",
        (false, _) if installed_by_setup => "absent — sera installé",
        (false, _) => "absent",
    }
}
fn is_home_edition(edition: &str) -> bool {
    let lower = edition.to_ascii_lowercase();
    lower.contains("home") || lower == "core" || lower.starts_with("core")
}

/// « Windows 11 Pro (build 26100) » : Windows 11 se déclare en version 10, on se fie au build.
fn windows_label(f: &SystemFacts) -> String {
    let name = match (f.windows_major, f.build) {
        (Some(10), Some(b)) if b >= 22000 => "Windows 11",
        (Some(10), _) => "Windows 10",
        _ => "Windows",
    };
    let edition = match f.edition.as_deref() {
        Some(e) if is_home_edition(e) => " Famille",
        Some(e) if e.to_ascii_lowercase().contains("pro") => " Pro",
        Some(e)
            if e.to_ascii_lowercase().contains("enterprise")
                || e.to_ascii_lowercase().contains("entreprise") =>
        {
            " Entreprise"
        }
        Some(_) => "",
        None => " (édition inconnue)",
    };
    match f.build {
        Some(b) => format!("{name}{edition} (build {b})"),
        None => format!("{name}{edition}"),
    }
}

pub fn evaluate(f: &SystemFacts) -> Vec<Check> {
    let mut out = Vec::new();
    macro_rules! check {
        ($id:expr,$title:expr,$status:expr,$found:expr,$expected:expr,$advice:expr) => {
            out.push(Check {
                id: $id,
                titre: $title,
                statut: $status,
                valeur_trouvee: $found,
                attendu: $expected,
                conseil: $advice,
            });
        };
    }
    let client_edition = f
        .edition
        .as_deref()
        .is_none_or(|e| !e.to_ascii_lowercase().contains("server"));
    // Un build/une édition inconnu(e) ne prouve jamais la compatibilité.
    let os = match (f.windows_major, f.build, f.edition.as_deref()) {
        (_, _, Some(e)) if is_home_edition(e) => Statut::Bloquant,
        (Some(10), Some(b), Some(e))
            if b >= 22631
                && (e.to_ascii_lowercase().contains("pro")
                    || e.to_ascii_lowercase().contains("enterprise")
                    || e.to_ascii_lowercase().contains("entreprise")) =>
        {
            Statut::Ok
        }
        (Some(10), Some(b), _) if b < 22000 => Statut::Attention, // Windows 10 : compatibilité non certifiée.
        _ => Statut::Attention,
    };
    check!(
        "windows",
        "Windows et édition",
        os,
        windows_label(f),
        "Windows 11 Pro/Entreprise, build 22631+",
        "Windows Famille ne permet pas les places. Les autres versions n'ont pas encore été testées avec Kinseat."
    );
    check!(
        "admin",
        "Droits administrateur",
        match f.admin {
            Some(true) => Statut::Ok,
            Some(false) => Statut::Bloquant,
            None => Statut::Attention,
        },
        yes_no(f.admin).into(),
        "Processus administrateur",
        "Les droits administrateur sont demandés à l'étape Installation."
    );
    // Les seuils sont stricts : exactement 10 Go n'est pas bloquant, exactement 30 Go est OK.
    check!(
        "disk",
        "Espace disque système",
        match f.disque_go {
            Some(n) if n < 10 => Statut::Bloquant,
            Some(n) if n < 30 => Statut::Attention,
            Some(_) => Statut::Ok,
            None => Statut::Attention,
        },
        f.disque_go.map_or("inconnu".into(), |n| format!("{n} Go")),
        "30 Go libres (10 Go minimum)",
        "Gardez au moins 10 Go libres sur le disque C:."
    );
    // Le fabricant seul n'atteste pas l'encodage matériel ; la validation réelle reste à faire.
    let gpu_ok = f.gpu.as_ref().is_some_and(|g| {
        ["nvidia", "amd", "intel"]
            .iter()
            .any(|v| g.to_ascii_lowercase().contains(v))
    });
    check!(
        "gpu",
        "GPU / encodage",
        if gpu_ok {
            Statut::Ok
        } else {
            Statut::Attention
        },
        f.gpu.clone().unwrap_or_else(|| "inconnue".into()),
        "GPU NVIDIA/AMD/Intel récent",
        "Carte graphique non reconnue : la diffusion fonctionnera peut-être plus lentement."
    );
    // Un port occupé ne provoque aucune interruption ni fermeture automatique.
    // Les ports tenus par Kinseat lui-même (mise à jour) ne sont pas un conflit.
    let ours = |p: &Port| {
        p.processus.as_deref().is_some_and(|n| {
            let n = n.to_ascii_lowercase();
            n.starts_with("multiseat.service") || n.starts_with("kinseat")
        })
    };
    let mut busy: Vec<String> = f
        .ports
        .iter()
        .filter(|p| (p.numero == 9550 || (47984..=48010).contains(&p.numero)) && !ours(p))
        .map(|p| {
            format!(
                "{}/{} ({})",
                p.numero,
                p.protocole,
                p.processus.as_deref().unwrap_or("processus inconnu")
            )
        })
        .collect();
    busy.dedup();
    check!(
        "ports",
        "Ports réseau de diffusion",
        if busy.is_empty() {
            if f.ports_known {
                Statut::Ok
            } else {
                Statut::Attention
            }
        } else {
            Statut::Attention
        },
        if busy.is_empty() {
            if f.ports_known {
                "libres".into()
            } else {
                "non vérifiés".into()
            }
        } else {
            busy.join(", ")
        },
        "9550 et 47984–48010 libres (TCP/UDP)",
        "Si un autre logiciel de diffusion (Sunshine, Apollo…) est ouvert, fermez-le avant de jouer avec Kinseat."
    );
    check!(
        "rdp_port",
        "Bureau à distance (port 3389)",
        Statut::Ok,
        if f.ports_known {
            if f.ports.iter().any(|p| p.numero == 3389) {
                "actif — sera limité à ce PC"
            } else {
                "inactif — sera activé pour ce PC seulement"
            }
        } else {
            "sera limité à ce PC"
        }
        .into(),
        "3389 non exposé sur le LAN",
        "Le pare-feu limitera le Bureau à distance à ce PC."
    );
    check!(
        "reboot",
        "Redémarrage en attente",
        if f.reboot_pending == Some(true) || f.reboot_pending.is_none() {
            Statut::Attention
        } else {
            Statut::Ok
        },
        match f.reboot_pending {
            Some(true) => "oui",
            Some(false) => "non",
            None => "inconnu",
        }
        .into(),
        "Aucun redémarrage en attente",
        "Si Windows attend un redémarrage, redémarrez le PC avant d'installer."
    );
    check!(
        "multiseat",
        "Installation précédente",
        Statut::Ok,
        if f.multiseat.present {
            "détectée — elle sera mise à jour"
        } else {
            "aucune"
        }
        .into(),
        "Pas d'installation à migrer",
        "Vos places et réglages sont conservés."
    );
    check!(
        "rdpwrap",
        "Plusieurs sessions Windows en même temps",
        if f.rdpwrap == Some(true) || !client_edition {
            Statut::Ok
        } else {
            Statut::Attention
        },
        match f.rdpwrap {
            Some(true) => "composant multi-session détecté",
            Some(false) if client_edition => "composant multi-session non détecté",
            Some(false) => "géré par cette édition de Windows",
            None => "inconnu",
        }
        .into(),
        "Sessions simultanées possibles",
        "Sur Windows Pro, jouer à plusieurs en même temps nécessite un composant multi-session (proposé à l'étape suivante)."
    );
    for (id, title, svc, installed_by_setup, advice) in [
        ("termservice", "Bureau à distance Windows", &f.term_service, false,
            "Service Windows « Services Bureau à distance » introuvable : les places ne pourront pas s'ouvrir."),
        ("vigem", "Manettes virtuelles (ViGEmBus)", &f.vigem_bus, false,
            "Pilote de manette virtuelle absent : les manettes ne fonctionneront pas dans les places. Installez ViGEmBus."),
        ("hidhide", "Isolation des manettes (HidHide)", &f.hid_hide, true, ""),
        ("apollo", "Diffusion (ApolloVibe)", &f.apollo, true, ""),
    ] {
        check!(
            id,
            title,
            if svc.present || installed_by_setup {
                Statut::Ok
            } else {
                Statut::Attention
            },
            service_label(svc, installed_by_setup).into(),
            "Présent",
            advice
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn base() -> SystemFacts {
        SystemFacts {
            windows_major: Some(10),
            build: Some(22631),
            edition: Some("Professional".into()),
            admin: Some(true),
            disque_go: Some(30),
            ports_known: true,
            gpu: Some("NVIDIA RTX".into()),
            reboot_pending: Some(false),
            rdpwrap: Some(false),
            ..Default::default()
        }
    }
    fn status(f: &SystemFacts, id: &str) -> Statut {
        evaluate(f).into_iter().find(|c| c.id == id).unwrap().statut
    }
    #[test]
    fn os() {
        let mut f = base();
        assert_eq!(status(&f, "windows"), Statut::Ok);
        f.edition = Some("Home".into());
        assert_eq!(status(&f, "windows"), Statut::Bloquant);
        f.edition = Some("Enterprise".into());
        f.build = Some(19045);
        assert_eq!(status(&f, "windows"), Statut::Attention);
        f.build = None;
        assert_eq!(status(&f, "windows"), Statut::Attention);
    }
    #[test]
    fn windows_home_core_is_blocking() {
        for edition in ["Core", "CoreSingleLanguage", "CoreN", "Home"] {
            let mut f = base();
            f.edition = Some(edition.into());
            assert_eq!(status(&f, "windows"), Statut::Bloquant, "edition={edition}");
        }
    }
    #[test]
    fn admin() {
        let mut f = base();
        f.admin = Some(false);
        assert_eq!(status(&f, "admin"), Statut::Bloquant);
    }
    #[test]
    fn disk() {
        let mut f = base();
        f.disque_go = Some(9);
        assert_eq!(status(&f, "disk"), Statut::Bloquant);
        f.disque_go = Some(10);
        assert_eq!(status(&f, "disk"), Statut::Attention);
        f.disque_go = Some(30);
        assert_eq!(status(&f, "disk"), Statut::Ok);
    }
    #[test]
    fn gpu() {
        let mut f = base();
        f.gpu = None;
        assert_eq!(status(&f, "gpu"), Statut::Attention);
        f.gpu = Some("Intel Arc".into());
        assert_eq!(status(&f, "gpu"), Statut::Ok);
    }
    #[test]
    fn ports() {
        let mut f = base();
        f.ports.push(Port {
            numero: 47984,
            protocole: "UDP".into(),
            processus: Some("Apollo.exe".into()),
        });
        assert_eq!(status(&f, "ports"), Statut::Attention);
        assert!(evaluate(&f)
            .iter()
            .find(|c| c.id == "ports")
            .unwrap()
            .valeur_trouvee
            .contains("Apollo.exe"));
        f.ports[0].numero = 3389;
        assert_eq!(status(&f, "ports"), Statut::Ok);
        // Le service Kinseat déjà installé n'est pas un conflit.
        f.ports[0].numero = 9550;
        f.ports[0].processus = Some("MultiSeat.Service.exe".into());
        assert_eq!(status(&f, "ports"), Statut::Ok);
    }
    #[test]
    fn pending_reboot() {
        let mut f = base();
        f.reboot_pending = Some(true);
        assert_eq!(status(&f, "reboot"), Statut::Attention);
    }
    #[test]
    fn previous_install() {
        let mut f = base();
        f.multiseat.present = true;
        assert_eq!(status(&f, "multiseat"), Statut::Ok);
    }
    #[test]
    fn rdpwrap() {
        let mut f = base();
        assert_eq!(status(&f, "rdpwrap"), Statut::Attention);
        f.rdpwrap = Some(true);
        assert_eq!(status(&f, "rdpwrap"), Statut::Ok);
        f.rdpwrap = Some(false);
        f.edition = Some("Windows Server 2022 Datacenter".into());
        assert_eq!(status(&f, "rdpwrap"), Statut::Ok);
    }
    #[test]
    fn services() {
        let mut f = base();
        f.term_service.present = true;
        assert_eq!(status(&f, "termservice"), Statut::Ok);
        assert_eq!(status(&f, "vigem"), Statut::Attention);
        // Installés par le setup : un absent n'est pas un problème.
        assert_eq!(status(&f, "hidhide"), Statut::Ok);
    }
    /// Plus de débogage Rust visible (« Some(false) », « present : true ») dans l'assistant.
    #[test]
    fn values_are_plain_french() {
        let mut f = base();
        f.build = Some(26100);
        f.hid_hide.present = true;
        f.hid_hide.actif = true;
        let checks = evaluate(&f);
        for c in &checks {
            for bad in ["Some(", "None", "true", "false"] {
                assert!(
                    !c.valeur_trouvee.contains(bad),
                    "{} : {}",
                    c.id,
                    c.valeur_trouvee
                );
            }
        }
        let value = |id: &str| {
            checks
                .iter()
                .find(|c| c.id == id)
                .unwrap()
                .valeur_trouvee
                .clone()
        };
        assert_eq!(value("windows"), "Windows 11 Pro (build 26100)");
        assert_eq!(value("hidhide"), "installé et démarré");
        assert_eq!(value("apollo"), "absent — sera installé");
    }
}
