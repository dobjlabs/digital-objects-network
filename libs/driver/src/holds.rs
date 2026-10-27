//! In-memory input holds prevent concurrent proofs from spending the same file.
//! Holds are released when dropped; open transaction records retain input
//! ownership after recording.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::error::DriverError;

/// Input filenames mapped to proof owner labels.
#[derive(Debug, Clone, Default)]
pub(crate) struct HoldTable {
    owners: Arc<Mutex<HashMap<String, String>>>,
}

/// Releases a proof's input holds when dropped.
#[derive(Debug)]
pub struct Holds {
    table: HoldTable,
    files: Vec<String>,
}

impl Holds {
    pub fn files(&self) -> &[String] {
        &self.files
    }
}

impl Drop for Holds {
    fn drop(&mut self) {
        let mut owners = self.table.owners.lock().expect("hold table poisoned");
        for file in &self.files {
            owners.remove(file);
        }
    }
}

impl HoldTable {
    /// Acquire all inputs or none. Refuse files already held or in `locked`,
    /// the input set of open transactions.
    pub(crate) fn take(
        &self,
        files: &[String],
        label: &str,
        locked: &HashSet<String>,
    ) -> Result<Holds, DriverError> {
        let mut owners = self.owners.lock().expect("hold table poisoned");
        for file in files {
            if let Some(owner) = owners.get(file) {
                return Err(DriverError::Conflict(format!(
                    "input {file} is held by {owner}"
                )));
            }
            if locked.contains(file) {
                return Err(DriverError::Conflict(format!(
                    "input {file} is spent by a transaction awaiting the chain"
                )));
            }
        }
        for file in files {
            owners.insert(file.clone(), label.to_string());
        }
        Ok(Holds {
            table: self.clone(),
            files: files.to_vec(),
        })
    }

    pub(crate) fn labels(&self) -> HashMap<String, String> {
        self.owners.lock().expect("hold table poisoned").clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_file_refuses_a_second_claim_until_released() {
        let table = HoldTable::default();
        let first = table
            .take(&["a.dobj".to_string()], "run-1", &HashSet::new())
            .unwrap();
        let err = table
            .take(
                &["b.dobj".to_string(), "a.dobj".to_string()],
                "run-2",
                &HashSet::new(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("held by run-1"), "{err}");
        assert!(!table.labels().contains_key("b.dobj"));
        drop(first);
        assert!(table.labels().is_empty());
        table
            .take(&["a.dobj".to_string()], "run-2", &HashSet::new())
            .unwrap();
    }

    #[test]
    fn a_locked_file_is_refused() {
        let table = HoldTable::default();
        let locked = HashSet::from(["a.dobj".to_string()]);
        let err = table
            .take(&["a.dobj".to_string()], "run-1", &locked)
            .unwrap_err();
        assert!(err.to_string().contains("awaiting the chain"), "{err}");
    }
}
