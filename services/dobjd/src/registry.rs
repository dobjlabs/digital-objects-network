//! Shared registry for runs and proof jobs, ordered by registration.
//! Finished entries remain available for polling until reaped.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// Retention period for finished entries.
pub const RETENTION: Duration = Duration::from_secs(600);
pub const REAP_INTERVAL: Duration = Duration::from_secs(60);

pub trait Retained: Send + Sync + 'static {
    /// Registration order, independent of timestamp ties.
    fn sequence(&self) -> u64;
    /// Completion time used to determine retention.
    fn finished_at(&self) -> Option<Instant>;

    fn is_finished(&self) -> bool {
        self.finished_at().is_some()
    }
}

pub struct Registry<E> {
    entries: Arc<RwLock<HashMap<String, Arc<E>>>>,
    next_sequence: Arc<AtomicU64>,
}

impl<E> Clone for Registry<E> {
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.clone(),
            next_sequence: self.next_sequence.clone(),
        }
    }
}

impl<E> Default for Registry<E> {
    fn default() -> Self {
        Self {
            entries: Arc::default(),
            next_sequence: Arc::default(),
        }
    }
}

impl<E: Retained> Registry<E> {
    /// Register an entry. The caller must supply a unique ID.
    pub fn start(&self, id: String, make: impl FnOnce(u64) -> E) -> Arc<E> {
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let entry = Arc::new(make(sequence));
        self.entries.write().unwrap().insert(id, entry.clone());
        entry
    }

    pub fn get(&self, id: &str) -> Option<Arc<E>> {
        self.entries.read().unwrap().get(id).cloned()
    }

    /// Retained entries in registration order, optionally limited to unfinished entries.
    pub fn entries(&self, active_only: bool) -> Vec<Arc<E>> {
        let mut entries: Vec<Arc<E>> = self
            .entries
            .read()
            .unwrap()
            .values()
            .filter(|entry| !(active_only && entry.is_finished()))
            .cloned()
            .collect();
        entries.sort_by_key(|entry| entry.sequence());
        entries
    }

    /// Remove entries whose retention period has elapsed.
    pub fn reap(&self, ttl: Duration) {
        self.entries.write().unwrap().retain(|_, entry| {
            !entry
                .finished_at()
                .is_some_and(|finished| finished.elapsed() >= ttl)
        });
    }
}
