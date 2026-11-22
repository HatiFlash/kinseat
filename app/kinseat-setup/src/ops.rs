//! Moteur transactionnel indépendant de Windows : journal persistant et reprise prudente.
//! Un état `applying` n'est jamais ré-exécuté automatiquement : une opération interrompue
//! peut avoir eu des effets ; l'opérateur doit d'abord la réconcilier.
use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Phase {
    Applying,
    Applied,
    Undoing,
    Undone,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub phase: Phase,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Journal {
    pub schema: u32,
    pub status: String,
    pub entries: Vec<Entry>,
}

impl Default for Journal {
    fn default() -> Self {
        Self {
            schema: 1,
            status: "en_cours".into(),
            entries: Vec::new(),
        }
    }
}

pub trait Operations {
    fn apply(&mut self, id: &str) -> Result<(), String>;
    fn verify(&mut self, id: &str) -> Result<(), String>;
    fn undo(&mut self, id: &str) -> Result<(), String>;
}

pub trait Store {
    fn load(&self) -> Result<Option<Journal>, String>;
    fn save(&mut self, journal: &Journal) -> Result<(), String>;
}

pub struct FileStore<'a>(pub &'a Path);
impl Store for FileStore<'_> {
    fn load(&self) -> Result<Option<Journal>, String> {
        match fs::read(self.0) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| e.to_string()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn save(&mut self, journal: &Journal) -> Result<(), String> {
        // Atomic rename on one volume, never truncate the only copy of the journal.
        let bytes = serde_json::to_vec_pretty(journal).map_err(|e| e.to_string())?;
        let tmp = self.0.with_extension("json.tmp");
        fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        fs::rename(&tmp, self.0).map_err(|e| e.to_string())
    }
}

pub fn rollback<O: Operations, S: Store>(
    ops: &mut O,
    store: &mut S,
    journal: &mut Journal,
) -> Result<(), String> {
    if journal.schema != 1 {
        return Err("Schéma de journal inconnu".into());
    }
    let mut failures = Vec::new();
    for i in (0..journal.entries.len()).rev() {
        if journal.entries[i].phase == Phase::Undone {
            continue;
        }
        let id = journal.entries[i].id.clone();
        // Une interruption dans apply/undo est indécidable : pas de suppression aveugle.
        if journal.entries[i].phase != Phase::Applied {
            failures.push(format!(
                "{id} : état incertain, intervention manuelle requise"
            ));
            continue;
        }
        journal.entries[i].phase = Phase::Undoing;
        store.save(journal)?;
        match ops.undo(&id) {
            Ok(()) => {
                journal.entries[i].phase = Phase::Undone;
                store.save(journal)?;
            }
            Err(e) => failures.push(format!("{id} : {e}")),
        }
    }
    journal.status = if failures.is_empty() {
        "annule".into()
    } else {
        "intervention_requise".into()
    };
    store.save(journal)?;
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join(" ; "))
    }
}

pub fn run<O: Operations, S: Store>(
    steps: &[String],
    ops: &mut O,
    store: &mut S,
) -> Result<Journal, String> {
    let mut journal = store.load()?.unwrap_or_default();
    if journal.schema != 1 || !matches!(journal.status.as_str(), "en_cours" | "termine") {
        return Err("Journal incompatible ou en attente d'intervention".into());
    }
    if journal.entries.len() > steps.len()
        || journal
            .entries
            .iter()
            .zip(steps)
            .any(|(entry, step)| entry.id != *step)
    {
        return Err("Plan différent du journal existant : reprise refusée".into());
    }
    if journal.entries.iter().any(|e| e.phase != Phase::Applied) {
        return Err("Opération interrompue : reprise automatique refusée".into());
    }
    for entry in &journal.entries {
        ops.verify(&entry.id)
            .map_err(|e| format!("Reprise impossible, {} : {e}", entry.id))?;
    }
    if journal.status == "termine" && journal.entries.len() != steps.len() {
        return Err("Journal terminé incomplet".into());
    }
    for step in steps.iter().skip(journal.entries.len()) {
        journal.entries.push(Entry {
            id: step.clone(),
            phase: Phase::Applying,
        });
        store.save(&journal)?;
        let result = ops.apply(step).and_then(|()| ops.verify(step));
        if let Err(e) = result {
            let recovery = rollback(ops, store, &mut journal);
            return Err(format!(
                "{step} : {e} ; retour arrière : {}",
                recovery.err().unwrap_or_else(|| "effectué".into())
            ));
        }
        journal.entries.last_mut().expect("entrée insérée").phase = Phase::Applied;
        store.save(&journal)?;
    }
    journal.status = "termine".into();
    store.save(&journal)?;
    Ok(journal)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Memory(Option<Journal>);
    impl Store for Memory {
        fn load(&self) -> Result<Option<Journal>, String> {
            Ok(self.0.clone())
        }
        fn save(&mut self, j: &Journal) -> Result<(), String> {
            self.0 = Some(j.clone());
            Ok(())
        }
    }
    #[derive(Default)]
    struct Fake {
        events: Vec<String>,
        fail: Option<String>,
    }
    impl Operations for Fake {
        fn apply(&mut self, id: &str) -> Result<(), String> {
            self.events.push(format!("do:{id}"));
            if self.fail.as_deref() == Some(id) {
                Err("panne".into())
            } else {
                Ok(())
            }
        }
        fn verify(&mut self, id: &str) -> Result<(), String> {
            self.events.push(format!("verify:{id}"));
            Ok(())
        }
        fn undo(&mut self, id: &str) -> Result<(), String> {
            self.events.push(format!("undo:{id}"));
            Ok(())
        }
    }
    #[test]
    fn failure_is_not_silently_undone_when_apply_is_uncertain() {
        let mut ops = Fake {
            fail: Some("c".into()),
            ..Fake::default()
        };
        let mut store = Memory::default();
        let err = run(&["a".into(), "b".into(), "c".into()], &mut ops, &mut store).unwrap_err();
        assert!(err.contains("intervention manuelle"));
        assert_eq!(
            ops.events,
            ["do:a", "verify:a", "do:b", "verify:b", "do:c", "undo:b", "undo:a"]
        );
        assert_eq!(store.0.unwrap().status, "intervention_requise");
    }
    #[test]
    fn resume_only_verified_prefix() {
        let mut store = Memory(Some(Journal {
            schema: 1,
            status: "en_cours".into(),
            entries: vec![Entry {
                id: "a".into(),
                phase: Phase::Applied,
            }],
        }));
        let mut ops = Fake::default();
        run(&["a".into(), "b".into()], &mut ops, &mut store).unwrap();
        assert_eq!(ops.events, ["verify:a", "do:b", "verify:b"]);
        assert_eq!(store.0.unwrap().status, "termine");
    }
    #[test]
    fn rejects_inflight_and_changed_plan() {
        let mut store = Memory(Some(Journal {
            schema: 1,
            status: "en_cours".into(),
            entries: vec![Entry {
                id: "a".into(),
                phase: Phase::Applying,
            }],
        }));
        let mut ops = Fake::default();
        assert!(run(&["a".into()], &mut ops, &mut store).is_err());
        store.0.as_mut().unwrap().entries[0].phase = Phase::Applied;
        assert!(run(&["b".into()], &mut ops, &mut store).is_err());
        assert!(ops.events.is_empty());
    }
}
