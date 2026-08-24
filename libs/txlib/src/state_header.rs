//! The committed view of app state a transaction grounds against.
//!
//! [`StateHeader`] is the commitments-only record the proof exposes;
//! [`GroundingWitness`] pairs it with the per-object Merkle proofs a
//! prover needs to show each input is in the global created set.

use std::sync::LazyLock;
use std::{collections::HashMap, sync::Arc};

use pod2::{
    backends::plonky2::primitives::merkletree::MerkleProof,
    middleware::{Hash, Value, containers::Array},
};
use serde::{Deserialize, Serialize};

/// Compact committed view of app state used for grounding transactions.
///
/// Holds only the Merkle roots needed to recompute the state
/// root hash and to verify synchronizer-supplied membership proofs. Full
/// containers are not carried -- callers prove each input's liveness with a
/// per-object Merkle proof packaged in a [`GroundingWitness`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateHeader {
    /// Execution block number
    pub block_number: i64,
    /// Execution block timestamp
    pub block_timestamp: i64,
    /// Execution block hash
    pub block_hash: Hash,
    /// Root of the global created-object set: the commitment of every object
    /// state ever created. Grounding proves an input is a member here.
    pub created_root: Hash,
    pub nullifiers_root: Hash,
    pub prior_state_history_root: Hash,
}

impl StateHeader {
    pub fn new(
        block_number: i64,
        block_timestamp: i64,
        block_hash: Hash,
        created_root: Hash,
        nullifiers_root: Hash,
        prior_state_history_root: Hash,
    ) -> Self {
        Self {
            block_number,
            block_timestamp,
            block_hash,
            created_root,
            nullifiers_root,
            prior_state_history_root,
        }
    }

    /// Array view used as the state root record. Slot layout
    /// matches the `record StateHeader` declaration in txlib.podlang.
    /// Predicates access fields via anchored-key syntax (e.g.
    /// `state_header.created`).
    pub fn array(&self) -> Array {
        Array::new(vec![
            Value::from(self.block_number),
            Value::from(self.block_timestamp),
            Value::from(self.block_hash),
            Value::from(self.created_root),
            Value::from(self.nullifiers_root),
            Value::from(self.prior_state_history_root),
        ])
    }

    /// Commitment of the state root array.
    pub fn hash(&self) -> Hash {
        self.array().commitment()
    }
}

/// Slot indices for the `StateHeader` record, matching the field order in
/// the `record StateHeader` declaration in txlib.podlang.
pub const STATE_HEADER_BLOCK_NUMBER_SLOT: usize = 0;
pub const STATE_HEADER_BLOCK_TIMESTAMP_SLOT: usize = 1;
pub const STATE_HEADER_BLOCK_HASH_SLOT: usize = 2;
pub const STATE_HEADER_CREATED_SLOT: usize = 3;
pub const STATE_HEADER_NULLIFIERS_SLOT: usize = 4;
pub const STATE_HEADER_PRIOR_STATE_HISTORY_SLOT: usize = 5;

pub static RECORD_STATE_HEADER_FIELDS: LazyLock<Arc<Vec<String>>> = LazyLock::new(|| {
    Arc::new(
        [
            "block_number",
            "block_timestamp",
            "block_hash",
            "created",
            "nullifiers",
            "prior_state_history",
        ]
        .map(|s| s.to_string())
        .into_iter()
        .collect::<Vec<_>>(),
    )
});

pub static RECORD_STATE_HEADER_PODLANG: LazyLock<String> = LazyLock::new(|| {
    let mut s = "record StateHeader = (".to_string();
    for (i, f) in RECORD_STATE_HEADER_FIELDS.iter().enumerate() {
        if i != 0 {
            s += ", ";
        }
        s += f;
    }
    s += ")";
    s
});

/// Proof-bearing grounding data required to build a new transaction.
///
/// Callers use `state_header` as the committed global context and
/// `created_proofs` to prove that each consumed input object is present in
/// `state_header.created_root` (the global created-object set). Proofs are keyed
/// by object commitment (`Dictionary::commitment()`) and carry the object's
/// array index, since grounding is `ArrayContains(created, index, obj)`. They
/// are fetched fresh at consume time because the created set grows: a proof is
/// only valid against the state root it was drawn from.
#[derive(Clone, Debug)]
pub struct GroundingWitness {
    pub state_header: StateHeader,
    /// Per-object `(index, Merkle proof)` for membership in the global created
    /// set, keyed by object commitment (`Dictionary::commitment()`).
    pub created_proofs: HashMap<Hash, (i64, MerkleProof)>,
}

impl GroundingWitness {
    pub fn new(
        state_header: StateHeader,
        created_proofs: HashMap<Hash, (i64, MerkleProof)>,
    ) -> Self {
        Self {
            state_header,
            created_proofs,
        }
    }
}
