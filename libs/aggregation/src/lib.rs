//! Recursive aggregation of ShrunkMainPod proofs.
//!
//! Each Aggregator step verifies `num_input_pods` shrunk proofs and, when continuing, the
//! previous Aggregator proof, and inserts the hash of each shrunk proof's public inputs into a
//! Merkle tree at the next free index. The final proof exposes the tree root, and a verifier
//! holding the list of public inputs rebuilds the root to check it.

mod aggregator;
mod circuit;

use std::collections::HashMap;

use plonky2::{hash::poseidon::PoseidonHash, plonk::config::Hasher};
use pod2::{
    backends::plonky2::primitives::merkletree::MerkleTree,
    middleware::{F, Hash, RawValue},
};

pub use aggregator::{AggregationState, Aggregator, verify_aggregation};
pub use circuit::{AggregatorInnerCircuit, AggregatorInput, AggregatorParams, AggregatorSlot};

/// Maximum number of shrunk proofs one aggregation chain can hold.
pub const MAX_AGG_SIZE: usize = 128;
/// Depth of the Merkle tree of leaves: the low bits of an index select its path.
pub const AGG_TREE_DEPTH: usize = 7;

/// Public inputs of an Aggregator proof: the tree root followed by the `verifier_data_hash` of
/// the Aggregator circuit itself.
pub const AGG_NUM_PUBLIC_INPUTS: usize = 8;
pub(crate) const AGG_ROOT_RANGE: std::ops::Range<usize> = 0..4;
pub(crate) const AGG_VERIFIER_DATA_HASH_RANGE: std::ops::Range<usize> = 4..8;

/// The leaf a shrunk proof contributes to the aggregation tree.
pub fn leaf_hash(shrunk_public_inputs: &[F]) -> Hash {
    Hash(PoseidonHash::hash_no_pad(shrunk_public_inputs).elements)
}

pub(crate) fn leaf_key(index: usize) -> RawValue {
    RawValue::from(index as i64)
}

/// The root of the tree holding `leaves` at indices `0..leaves.len()`.
pub fn aggregation_root(leaves: &[Hash]) -> Hash {
    let kvs: HashMap<RawValue, RawValue> = leaves
        .iter()
        .enumerate()
        .map(|(index, leaf)| (leaf_key(index), RawValue(leaf.0)))
        .collect();
    MerkleTree::new(&kvs).root()
}

#[cfg(test)]
mod tests {
    use pod2::middleware::EMPTY_HASH;

    use super::*;

    #[test]
    fn test_tree_depth_matches_max_size() {
        assert_eq!(1 << AGG_TREE_DEPTH, MAX_AGG_SIZE);
    }

    #[test]
    fn test_aggregation_root() {
        assert_eq!(aggregation_root(&[]), EMPTY_HASH);

        let leaves: Vec<Hash> = (0..5u64)
            .map(|index| leaf_hash(&[F(index), F(index + 1)]))
            .collect();
        let mut tree = MerkleTree::new(&HashMap::new());
        for (index, leaf) in leaves.iter().enumerate() {
            tree.insert(&leaf_key(index), &RawValue(leaf.0)).unwrap();
        }
        assert_eq!(aggregation_root(&leaves), tree.root());
    }
}
