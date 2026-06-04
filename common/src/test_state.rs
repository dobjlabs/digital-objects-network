use std::collections::{HashMap, HashSet};

use pod2::{
    backends::plonky2::primitives::merkletree::MerkleProof,
    middleware::{
        Hash, Value,
        containers::{Array, Set},
    },
};

/// Reusable committed-state helper for proof-heavy tests across crates. Holds
/// a grow-only global created-object set (an array, plus a reverse index for
/// proofs), a nullifier set, and a GSR history array, and hands out the Merkle
/// proofs grounding needs.
#[derive(Clone, Debug)]
pub struct TestState {
    pub block_number: i64,
    created: Array,
    created_index: HashMap<Hash, i64>,
    nullifiers: Set,
    gsrs: Array,
}

impl Default for TestState {
    fn default() -> Self {
        Self::empty(0)
    }
}

impl TestState {
    pub fn empty(block_number: i64) -> Self {
        Self {
            block_number,
            created: Array::new(Vec::<Value>::new()),
            created_index: HashMap::new(),
            nullifiers: Set::new(HashSet::<Value>::new()),
            gsrs: Array::new(Vec::<Value>::new()),
        }
    }

    /// `(created_root, nullifiers_root, gsrs_root)`.
    pub fn roots(&self) -> (Hash, Hash, Hash) {
        (
            self.created.commitment(),
            self.nullifiers.commitment(),
            self.gsrs.commitment(),
        )
    }

    /// Build a grounding witness proving each input object commitment is a
    /// member of the global created set. `build` assembles the crate's witness
    /// type from `(block_number, created_root, nullifiers_root, gsrs_root,
    /// per-object (index, proof) keyed by commitment)`.
    pub fn build_grounding_witness<W>(
        &self,
        input_commitments: &[Hash],
        build: impl FnOnce(i64, Hash, Hash, Hash, HashMap<Hash, (i64, MerkleProof)>) -> W,
    ) -> W {
        let created_proofs = input_commitments
            .iter()
            .map(|commitment| (*commitment, self.created_membership_proof(*commitment)))
            .collect::<HashMap<_, _>>();
        let (created_root, nullifiers_root, gsrs_root) = self.roots();
        build(
            self.block_number,
            created_root,
            nullifiers_root,
            gsrs_root,
            created_proofs,
        )
    }

    /// `(array index, membership proof)` for one object commitment in the
    /// created set.
    pub fn created_membership_proof(&self, commitment: Hash) -> (i64, MerkleProof) {
        let index = *self
            .created_index
            .get(&commitment)
            .expect("object should be present in test state created set");
        let (_value, proof) = self
            .created
            .prove(index as usize)
            .expect("object should be provable from test state");
        (index, proof)
    }

    pub fn prior_state_root_membership(&self, prior_state_root_hash: Hash) -> (usize, MerkleProof) {
        let target = Value::from(prior_state_root_hash);
        for entry in self.gsrs.iter() {
            let (index, value) = entry.expect("gsr entry should decode");
            if value == target {
                let (_, proof) = self.gsrs.prove(index).expect("gsr proof should build");
                return (index, proof);
            }
        }
        panic!("prior state root missing from grounding state");
    }

    pub fn apply_tx(
        &mut self,
        created_commitments: impl IntoIterator<Item = Hash>,
        nullifier_hashes: impl IntoIterator<Item = Hash>,
    ) {
        for commitment in created_commitments {
            // 1-indexed: slot 0 stays empty so nothing grounds at index 0.
            let index = self.created_index.len() as i64 + 1;
            self.created
                .insert(index as usize, Value::from(commitment))
                .expect("created object should insert into test state");
            self.created_index.insert(commitment, index);
        }
        for nullifier in nullifier_hashes {
            self.nullifiers
                .insert(&Value::from(nullifier))
                .expect("nullifier should insert into test state");
        }
        self.block_number += 1;
    }
}
