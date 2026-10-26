//! In-memory input holds prevent concurrent proofs from spending the same file.
//! Holds are released when dropped; open transaction records retain input
//! ownership after recording.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::error::DriverError;

#[derive(Debug, Clone)]
pub(crate) struct HoldOwner {
    hold_id: u64,
    pub(crate) label: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct HoldTable {
    inner: Arc<Mutex<HoldTableInner>>,
}

#[derive(Debug, Default)]
struct HoldTableInner {
    next_hold_id: u64,
    owners: HashMap<String, HoldOwner>,
}

/// Releases a proof's input holds when dropped.
#[derive(Debug)]
pub struct Holds {
    table: HoldTable,
    hold_id: u64,
    files: Vec<String>,
}

impl Holds {
    pub fn files(&self) -> &[String] {
        &self.files
    }
}

impl Drop for Holds {
    fn drop(&mut self) {
        let mut inner = self.table.inner.lock().expect("hold table poisoned");
        inner
            .owners
            .retain(|_, owner| owner.hold_id != self.hold_id);
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
        let mut inner = self.inner.lock().expect("hold table poisoned");
        for file in files {
            if let Some(owner) = inner.owners.get(file) {
                return Err(DriverError::Conflict(format!(
                    "input {file} is held by {}",
                    owner.label
                )));
            }
            if locked.contains(file) {
                return Err(DriverError::Conflict(format!(
                    "input {file} is spent by a transaction awaiting the chain"
                )));
            }
        }
        let hold_id = inner.next_hold_id;
        inner.next_hold_id += 1;
        for file in files {
            inner.owners.insert(
                file.clone(),
                HoldOwner {
                    hold_id,
                    label: label.to_string(),
                },
            );
        }
        Ok(Holds {
            table: self.clone(),
            hold_id,
            files: files.to_vec(),
        })
    }

    /// Basename to the label of the proof holding it.
    pub(crate) fn labels(&self) -> HashMap<String, String> {
        self.inner
            .lock()
            .expect("hold table poisoned")
            .owners
            .iter()
            .map(|(file, owner)| (file.clone(), owner.label.clone()))
            .collect()
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
