//! Two-level grounding verification.
//!
//! For each consumed input the guest must prove:
//!
//! 1. `obj.commitment ∈ source_tx.live_root` — the source tx really did
//!    create this object.
//! 2. `source_tx_final ∈ state_root.transactions_root` — the source tx
//!    was finalized.
//!
//! Then it binds the two by *recomputing* `source_tx_final` from
//! `(source_tx_action_id, source_tx_live_root, source_tx_nullifiers_root,
//! source_tx_action_nonce)`. Without this, a prover could pass a forged
//! `source_tx_final` together with valid Merkle proofs against unrelated
//! trees.
//!
//! All inputs in one action ground against the same `state_root` — there's no
//! per-input root choice, so a malicious prover can't pick different
//! `transactions_root`s per input.

use txlib_core::Hash;
use txlib_core::abi::{GuestInput, InputObject};
use txlib_core::merkle::verify_inclusion;
use txlib_core::tx::Tx;

/// Verify grounding for every input. Panics on any failure.
pub fn verify_all(input: &GuestInput) {
    let transactions_root = input.state_root.transactions_root;
    for (i, inp) in input.inputs.iter().enumerate() {
        verify_one(transactions_root, inp).unwrap_or_else(|reason| {
            panic!("input {i} failed grounding: {reason}");
        });
    }
}

fn verify_one(transactions_root: Hash, inp: &InputObject) -> Result<(), &'static str> {
    let obj_commitment = inp.obj.commitment();

    if !verify_inclusion(
        inp.source_tx_live_root,
        obj_commitment,
        obj_commitment,
        &inp.live_inclusion_proof,
    ) {
        return Err("obj commitment not in source_tx.live_root");
    }

    let source_tx = Tx {
        action_id: inp.source_tx_action_id,
        live_root: inp.source_tx_live_root,
        nullifiers_root: inp.source_tx_nullifiers_root,
        action_nonce: inp.source_tx_action_nonce,
    };
    let source_tx_final = source_tx.tx_final();

    if !verify_inclusion(
        transactions_root,
        source_tx_final,
        source_tx_final,
        &inp.tx_inclusion_proof,
    ) {
        return Err("source_tx_final not in transactions_root");
    }

    Ok(())
}
