//! Plan explicite sérialisé avant toute élévation.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, fs::File, io::Read, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub version: String,
    pub file: String,
    pub sha256: String,
    pub required: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub artifacts: Vec<Artifact>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub install_dir: String,
    pub apollo: bool,
    pub virtual_display: bool,
    pub rdpwrap: bool,
    pub desktop_shortcut: bool,
    pub rdp_consent: bool,
    pub keep_network_rdp: bool,
    pub defender_exclusion: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            install_dir: r"C:\Program Files\Kinseat".into(),
            apollo: true,
            virtual_display: false,
            rdpwrap: false,
            desktop_shortcut: true,
            rdp_consent: false,
            keep_network_rdp: false,
            defender_exclusion: false,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub options: Options,
    pub steps: Vec<String>,
}
pub const MULTISESSION_STEP: &str =
    "Activer plusieurs sessions Windows en même temps (RDP Wrapper, expérimental)";
/// Case « plusieurs sessions » : le consentement distinct suit la case, jamais l'inverse.
pub fn set_multisession(options: &mut Options, enabled: bool) {
    options.rdpwrap = enabled;
    options.rdp_consent = enabled;
}
pub fn plan(options: Options) -> Result<Plan, String> {
    if options.install_dir != r"C:\Program Files\Kinseat" {
        return Err("Dossier imposé : C:\\Program Files\\Kinseat".into());
    }
    if options.rdpwrap && !options.rdp_consent {
        return Err("RDPWrap exige un consentement distinct".into());
    }
    let mut steps = vec![
        "Vérifier l'intégrité des fichiers à installer".into(),
        "Sauvegarder l'installation existante s'il y en a une".into(),
        "Installer le service Kinseat et l'application Hôte".into(),
        "Préparer le Bureau à distance local utilisé par les places".into(),
        if options.keep_network_rdp {
            "Laisser le Bureau à distance accessible depuis le réseau (risque élevé)"
        } else {
            "Pare-feu : Bureau à distance limité à ce PC"
        }
        .into(),
        "Démarrer le service Kinseat".into(),
    ];
    if options.apollo {
        steps.push("Installer ApolloVibe (diffusion du jeu)".into());
    }
    if options.defender_exclusion {
        steps.push("Ajouter une exclusion Defender explicite".into());
    }
    if options.virtual_display {
        steps.push("Installer l'écran virtuel".into());
    }
    if options.rdpwrap {
        steps.push(MULTISESSION_STEP.into());
    }
    steps.push("Installer HidHide (chaque manette reste sur sa place)".into());
    if options.desktop_shortcut {
        steps.push("Créer les raccourcis Bureau et menu Démarrer".into());
    }
    steps.extend([
        "Inscrire Kinseat dans « Applications installées »".into(),
        "Vérifier que tout fonctionne".into(),
    ]);
    Ok(Plan { options, steps })
}
/// Ne pas suivre les jonctions/symlinks d'un payload externe ; vérification avant toute élévation.
pub fn verify_manifest(manifest: &Manifest, root: &Path) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    if manifest.artifacts.is_empty() {
        return Err("Manifeste vide".into());
    }
    let mut verified = Vec::new();
    for a in &manifest.artifacts {
        let path = Path::new(&a.file);
        if path.is_absolute()
            || a.file.contains(':')
            || path
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
            || !seen.insert(a.file.to_ascii_lowercase())
        {
            return Err(format!(
                "Chemin de charge utile interdit ou en double : {}",
                a.file
            ));
        }
        if a.sha256.len() != 64 || !a.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!(
                "Empreinte SHA-256 non qualifiée pour {} ({})",
                a.id, a.version
            ));
        }
        let full = root.join(path);
        if full
            .symlink_metadata()
            .map_err(|e| format!("{} : {e}", a.id))?
            .file_type()
            .is_symlink()
        {
            return Err(format!("Lien symbolique interdit : {}", a.id));
        }
        let mut file = File::open(&full).map_err(|e| format!("{} : {e}", a.id))?;
        let mut hash = Sha256::new();
        let mut buf = [0u8; 65536];
        loop {
            let n = file.read(&mut buf).map_err(|e| format!("{} : {e}", a.id))?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        let actual = format!("{:x}", hash.finalize());
        if !actual.eq_ignore_ascii_case(&a.sha256) {
            return Err(format!(
                "Empreinte incorrecte : {} attendu {} obtenu {}",
                a.id, a.sha256, actual
            ));
        }
        verified.push(format!("{} {} SHA-256 {}", a.id, a.version, actual));
    }
    Ok(verified)
}
#[allow(dead_code)] // format du journal d’installation, lu par la désinstallation
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Change {
    pub id: String,
    pub before: Option<String>,
    pub after: String,
    pub created_by_kinseat: bool,
}
#[allow(dead_code)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstallState {
    pub schema_version: u32,
    pub status: String,
    pub changes: Vec<Change>,
}
/// Une valeur modifiée par un tiers depuis notre intervention ne doit jamais être écrasée.
#[cfg(test)]
pub fn restore<'a>(
    state: &'a InstallState,
    id: &str,
    current: Option<&str>,
) -> Result<Option<&'a str>, &'static str> {
    if state.schema_version != 1 {
        return Err("Schéma inconnu");
    }
    let change = state
        .changes
        .iter()
        .find(|c| c.id == id)
        .ok_or("Changement non possédé")?;
    if !change.created_by_kinseat {
        return Err("Changement préexistant");
    }
    if current != Some(change.after.as_str()) {
        return Err("Conflit : valeur modifiée depuis l'installation");
    }
    Ok(change.before.as_deref())
}
#[cfg(test)]
pub fn execute_simulated<F, R>(
    steps: &[String],
    mut apply: F,
    mut rollback: R,
) -> Result<(), String>
where
    F: FnMut(&str) -> Result<(), String>,
    R: FnMut(&str) -> Result<(), String>,
{
    let mut done = Vec::new();
    for step in steps {
        if let Err(e) = apply(step) {
            let failures: Vec<_> = done
                .into_iter()
                .rev()
                .filter_map(|s| rollback(s).err())
                .collect();
            return Err(format!(
                "{step} : {e} ; restauration : {}",
                if failures.is_empty() {
                    "effectuée".into()
                } else {
                    failures.join(" ; ")
                }
            ));
        }
        done.push(step.as_str());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn consent_required() {
        let mut o = Options {
            rdpwrap: true,
            ..Options::default()
        };
        assert!(plan(o.clone()).is_err());
        o.rdp_consent = true;
        assert!(plan(o).is_ok());
    }
    #[test]
    fn multisession_off_by_default_and_explicit() {
        let o = Options::default();
        assert!(!o.rdpwrap && !o.rdp_consent);
        let p = plan(o.clone()).unwrap();
        assert!(!p.steps.iter().any(|s| s == MULTISESSION_STEP));
        let mut on = o;
        set_multisession(&mut on, true);
        let p = plan(on.clone()).unwrap();
        assert_eq!(
            p.steps.iter().filter(|s| *s == MULTISESSION_STEP).count(),
            1
        );
        set_multisession(&mut on, false);
        assert!(!on.rdpwrap && !on.rdp_consent);
    }
    #[test]
    fn manifest_fail_closed() {
        let mut m = Manifest {
            version: "0.3.11".into(),
            artifacts: vec![Artifact {
                id: "test".into(),
                version: "1".into(),
                file: "a".into(),
                sha256: "A_RENSEIGNER".into(),
                required: true,
            }],
        };
        assert!(verify_manifest(&m, Path::new("."))
            .unwrap_err()
            .contains("non qualifiée"));
        m.artifacts[0].file = "../secret".into();
        assert!(verify_manifest(&m, Path::new(".")).is_err());
        m.artifacts[0].file = "Cargo.toml".into();
        m.artifacts[0].sha256 =
            format!("{:x}", Sha256::digest(std::fs::read("Cargo.toml").unwrap()));
        assert!(verify_manifest(&m, Path::new(".")).is_ok());
        m.artifacts[0].sha256 = "0".repeat(64);
        assert!(verify_manifest(&m, Path::new(".")).is_err());
    }
    #[test]
    fn rollback_reverse_and_conflict() {
        let steps = vec!["a".into(), "b".into(), "c".into()];
        let mut events = vec![];
        let err = execute_simulated(
            &steps,
            |s| {
                if s == "c" {
                    Err("échec".into())
                } else {
                    Ok(())
                }
            },
            |s| {
                events.push(s.to_string());
                Ok(())
            },
        )
        .unwrap_err();
        assert!(err.contains("restauration"));
        assert_eq!(events, ["b", "a"]);
        let state = InstallState {
            schema_version: 1,
            status: "complete".into(),
            changes: vec![Change {
                id: "service".into(),
                before: Some("old".into()),
                after: "new".into(),
                created_by_kinseat: true,
            }],
        };
        assert_eq!(restore(&state, "service", Some("new")), Ok(Some("old")));
        assert!(restore(&state, "service", Some("foreign")).is_err());
    }
}
