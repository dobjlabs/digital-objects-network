//! Host ↔ guest ABI for the unified risc0 action guest.
//!
//! The driver builds a [`GuestInput`], borsh-encodes it, and feeds it to the
//! risc0 prover via `env::write`. The guest reads it via `env::read`,
//! validates everything, runs the action's predicate, and commits a
//! [`GuestJournal`] via `env::commit`.
//!
//! ## What the journal commits
//! Exactly the three values the synchronizer needs to advance canonical state:
//! - `state_root_hash` — the GSR this tx was grounded against
//! - `tx_final` — the new tx's commitment, to be inserted into the
//!   `transactions` set
//! - `nullifiers` — the consumed objects' nullifiers, to be inserted into
//!   the `nullifiers` set
//!
//! Everything else is private. The synchronizer reconstructs the public
//! statement from the journal bytes and verifies the receipt against the
//! pinned `image_id` for the unified guest.
//!
//! ## What the guest is responsible for
//! The guest's `main()` must enforce, for each action invocation:
//! 1. Every input object's `source_tx_final` is in `state_root.transactions_root`
//!    (via [`InputObject::tx_inclusion_proof`]).
//! 2. Every input object's commitment is in `source_tx.live_root`
//!    (via [`InputObject::live_inclusion_proof`]).
//! 3. The action's predicate over `(inputs, new_objects, intro_witnesses)`
//!    holds (action-specific, dispatched by `action_id`).
//! 4. Each output object's commitment is in the new tx's `live_root`.
//! 5. Each input object's nullifier is in the new tx's `nullifiers_root`.
//! 6. The committed `tx_final` matches the recomputed `Tx::tx_final()`.

use alloc::vec::Vec;
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use crate::hash::Hash;
use crate::merkle::MerkleProof;
use crate::object::Object;
use crate::tx::StateRoot;

/// Action identifier — namespaces the dispatcher and is committed inside
/// `tx_final`. Action IDs are assigned at guest build time and form the
/// public ABI between the driver and the guest binary.
pub type ActionId = u32;

/// Input to one action invocation. All hashes must be SHA-256.
#[derive(
    Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct GuestInput {
    /// Which action to dispatch. Must be one the guest knows about.
    pub action_id: ActionId,
    /// The grounding state root. The guest verifies every input against this.
    pub state_root: StateRoot,
    /// Objects being consumed. Empty for actions that create from nothing
    /// (e.g. `FindLog`).
    pub inputs: Vec<InputObject>,
    /// Objects being produced. Each carries a fresh random `key` field.
    pub new_objects: Vec<Object>,
    /// Witness data for any intro-style sub-protocols (VDF outputs, etc.).
    /// Indexed by the action; the guest knows which slots it expects.
    pub intro_witnesses: Vec<IntroWitness>,
}

/// One consumed object plus the two grounding proofs needed to anchor it.
///
/// The guest verifies, in order:
/// 1. `live_inclusion_proof`: `obj.commitment()` is in `source_tx_live_root`,
///    proving the object was created by `source_tx`.
/// 2. `tx_inclusion_proof`: `source_tx_final` is in
///    `state_root.transactions_root`, proving `source_tx` was finalized.
/// 3. Recomputes `source_tx_final` from `(action_id, source_tx_live_root,
///    source_tx_nullifiers_root, source_tx_action_nonce)` and confirms it
///    matches what the inclusion proof verified.
#[derive(
    Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct InputObject {
    pub obj: Object,

    // --- source tx components, needed to recompute source tx_final ---
    pub source_tx_action_id: ActionId,
    pub source_tx_live_root: Hash,
    pub source_tx_nullifiers_root: Hash,
    pub source_tx_action_nonce: Hash,

    // --- grounding proofs ---
    /// Proves `obj.commitment()` is in `source_tx_live_root`.
    pub live_inclusion_proof: MerkleProof,
    /// Proves `source_tx_final` is in `state_root.transactions_root`.
    pub tx_inclusion_proof: MerkleProof,
}

/// Public journal output. The synchronizer parses these bytes from the
/// receipt's journal and uses them to advance canonical state.
#[derive(
    Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct GuestJournal {
    pub state_root_hash: Hash,
    pub tx_final: Hash,
    pub nullifiers: Vec<Hash>,
}

/// Free-form per-action witness slot. Each variant is a self-contained
/// witness for one intro-style check; the guest's action handler decides
/// which variants it needs and in what order.
#[derive(
    Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub enum IntroWitness {
    /// A SHA-256 chain of length `iters` starting at `input` ending at `output`.
    /// The guest re-runs the chain to verify (`iters` SHA-256 calls).
    /// Replaces the pod2-era `vdfpod`.
    Vdf {
        iters: u32,
        input: Hash,
        output: Hash,
    },
    /// 32-byte big-endian comparison: `lhs <= rhs`. Replaces the pod2-era
    /// `lt_eq_u256_pod`. Witness-free — included only as an explicit
    /// declaration of intent, the guest checks the comparison directly.
    LtEqU256 { lhs: [u8; 32], rhs: [u8; 32] },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::sha256;
    use crate::merkle::SMT_DEPTH;
    use crate::object;

    fn dummy_proof() -> MerkleProof {
        MerkleProof {
            siblings: alloc::vec![Hash::default(); SMT_DEPTH],
        }
    }

    #[test]
    fn guest_input_borsh_roundtrip() {
        let key = sha256(b"k");
        let obj = object! {
            "blueprint" => "Wood",
            "key" => key,
            "durability" => 100i64,
        };
        let input = GuestInput {
            action_id: 7,
            state_root: StateRoot::new(
                42,
                Hash([1; 32]),
                Hash([2; 32]),
                Hash([3; 32]),
            ),
            inputs: alloc::vec![InputObject {
                obj: obj.clone(),
                source_tx_action_id: 3,
                source_tx_live_root: Hash([4; 32]),
                source_tx_nullifiers_root: Hash([5; 32]),
                source_tx_action_nonce: Hash([6; 32]),
                live_inclusion_proof: dummy_proof(),
                tx_inclusion_proof: dummy_proof(),
            }],
            new_objects: alloc::vec![obj.clone()],
            intro_witnesses: alloc::vec![IntroWitness::Vdf {
                iters: 10,
                input: sha256(b"v_in"),
                output: sha256(b"v_out"),
            }],
        };

        let bytes = borsh::to_vec(&input).unwrap();
        let decoded: GuestInput = borsh::from_slice(&bytes).unwrap();
        assert_eq!(input, decoded);
    }

    #[test]
    fn guest_journal_borsh_roundtrip() {
        let j = GuestJournal {
            state_root_hash: sha256(b"sr"),
            tx_final: sha256(b"tf"),
            nullifiers: alloc::vec![sha256(b"n1"), sha256(b"n2")],
        };
        let bytes = borsh::to_vec(&j).unwrap();
        let decoded: GuestJournal = borsh::from_slice(&bytes).unwrap();
        assert_eq!(j, decoded);
    }

    #[test]
    fn intro_witness_variants_roundtrip() {
        for w in [
            IntroWitness::Vdf {
                iters: 5,
                input: sha256(b"in"),
                output: sha256(b"out"),
            },
            IntroWitness::LtEqU256 {
                lhs: [0xab; 32],
                rhs: [0xcd; 32],
            },
        ] {
            let bytes = borsh::to_vec(&w).unwrap();
            let decoded: IntroWitness = borsh::from_slice(&bytes).unwrap();
            assert_eq!(w, decoded);
        }
    }
}
