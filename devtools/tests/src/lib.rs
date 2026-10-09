//! Shared pieces of the live-network test binaries: proving actions of this crate's plugins
//! into relayer payloads, and talking to the relayer and synchronizer named in `.env`.

mod prover;
mod services;

pub use prover::{PreparedTx, Prover, find_log_action};
pub use services::{Services, SubmittedTx};
