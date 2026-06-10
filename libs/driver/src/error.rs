//! Typed errors for cases where callers (in particular HTTP clients via
//! `dobjd`) need a status-code mapping richer than 500. The driver still
//! returns `anyhow::Result` everywhere — these variants get wrapped via
//! `anyhow::Error::from(DriverError::…)`, and consumers downcast with
//! `err.downcast_ref::<DriverError>()` to pick a status code.
//!
//! Keep this enum small. Add a variant only when an HTTP caller can sensibly
//! act on it differently from a generic 500 (e.g., distinguishing a missing
//! object from a corrupt RocksDB).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DriverError {
    #[error("unknown action: {0}")]
    UnknownAction(String),

    #[error("unknown class: {0}")]
    UnknownClass(String),

    #[error("object not found: {0}")]
    ObjectNotFound(String),

    #[error("object file not found: {0}")]
    ObjectFileNotFound(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("conflict: {0}")]
    Conflict(String),
}
