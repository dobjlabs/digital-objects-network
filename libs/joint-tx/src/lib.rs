//! Assembling one transaction across parties that hold different
//! pieces of it.
//!
//! txlib below this defines what a transaction is and how one party
//! builds and finalizes it. It is party-blind: a mutation side there
//! says whether the builder can open a state, never whose it is. This
//! crate owns the party, and holds the three things that only exist
//! because the private data is split across several of them:
//!
//! - `contribute` -- what a holder proves about its own objects so
//!   another party can assemble over them, and the receive-side
//!   validation that rejects a mismatched bundle at the wire rather
//!   than as an opaque solver failure. Every one of these is produced
//!   with only a `BuildContext`: contributing to a transaction does
//!   not mean building one.
//! - `plan` -- the effect itself, derived identically by every party
//!   from commitments alone. Completing it is the single data round
//!   that precedes all proving, since endorsements bind `tx_final`.
//! - `graph` -- which party can prove what, and therefore the order
//!   they go in. The schedule is the protocol.
//!
//! Every spend is endorsed by its owner against the whole effect, so
//! nothing finalizes until every consumed object's owner has agreed to
//! this exact transaction, and any edit invalidates every endorsement.
//! That property is txlib's, not this crate's: it holds for a
//! single-party transaction too, which is why the spend rule stayed
//! below and only its cross-party plumbing lives here.

mod contribute;
pub mod graph;
mod plan;

pub use contribute::{ObjectOpenings, SpendAuthorization, TransferAcceptance, TransferOffer};
pub use plan::{PlannedEvent, TxPlan};

#[cfg(test)]
mod tests;
