//! Build the new transaction's commitments + the journal the guest commits.
//!
//! Steps:
//! 1. Compute one nullifier per input (must come BEFORE dispatch, since the
//!    journal commits to them).
//! 2. After dispatch validates the action, compute `live_root` (SMT over the
//!    new objects' commitments) and `nullifiers_root`.
//! 3. Build the per-invocation `action_nonce` (hash of action_id + sorted new
//!    object commitments — uniqueness comes from each output's random `key`).
//! 4. Assemble the `Tx`, compute `tx_final`, package up the journal.

use alloc::vec::Vec;

use txlib_core::Hash;
use txlib_core::abi::{GuestInput, GuestJournal};
use txlib_core::merkle::set_smt_root;
use txlib_core::tx::{Tx, action_nonce, compute_nullifier};

pub fn nullifiers_for(input: &GuestInput) -> Vec<Hash> {
    input
        .inputs
        .iter()
        .map(|i| compute_nullifier(&i.obj))
        .collect()
}

/// Sorted commitments of the new objects — canonical order for `live_root` /
/// `action_nonce` derivation. Reused by both the guest pipeline and the
/// driver (which needs these to populate output record `source_tx_live`).
pub fn sorted_new_obj_commitments(input: &GuestInput) -> Vec<Hash> {
    let mut v: Vec<Hash> = input.new_objects.iter().map(|o| o.commitment()).collect();
    v.sort();
    v
}

/// Assemble the [`Tx`] this action produces. Caller already has the
/// nullifier list (typically from [`nullifiers_for`]).
pub fn build_tx(input: &GuestInput, nullifiers: &[Hash]) -> Tx {
    let new_obj_commitments = sorted_new_obj_commitments(input);
    Tx {
        action_id: input.action_id,
        live_root: set_smt_root(&new_obj_commitments),
        nullifiers_root: set_smt_root(nullifiers),
        action_nonce: action_nonce(input.action_id, &new_obj_commitments),
    }
}

pub fn build_journal(input: &GuestInput, nullifiers: Vec<Hash>) -> GuestJournal {
    let tx = build_tx(input, &nullifiers);
    GuestJournal {
        state_root_hash: input.state_root.hash(),
        tx_final: tx.tx_final(),
        nullifiers,
    }
}
