//! Assemble one transaction from proofs produced by multiple parties.
//!
//! Each state holder proves the facts that require its private data. A
//! designated finalizer combines those contributions into one transaction.
//!
//! - `transaction` validates the participants, finalizer, labeled events,
//!   and final key custody.
//! - `contribute` defines proof bundles and validates received bundles.
//! - `plan` derives chain positions, scopes, sets, and `tx_final` from
//!   commitments.
//! - `graph` derives statement dependencies and the proving schedule.
//!
//! Every spend endorses the exact transaction context, so finalization
//! requires authorization from every consumed state's key holder.

mod contribute;
pub mod graph;
mod plan;
mod transaction;

pub use contribute::{ObjectOpenings, SpendAuthorization, TransferAcceptance, TransferOffer};
pub use plan::{PlannedEvent, TxPlan};
pub use transaction::{InputState, JointEvent, JointTransaction, KeyAnnotation};

#[cfg(test)]
mod tests;
