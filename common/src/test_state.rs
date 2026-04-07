use std::collections::{HashMap, HashSet};

use pod2::{
    backends::plonky2::primitives::merkletree::MerkleProof,
    middleware::{
        EMPTY_HASH, Hash, Value,
        containers::{Array, Dictionary, Set},
    },
};

/// Reusable committed-state helper for proof-heavy tests across crates.
#[derive(Clone, Debug)]
pub struct TestState {
    pub block_number: i64,
    transactions: Set,
    nullifiers: Set,
    gsrs: Array,
    public_objects: Dictionary,
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
            transactions: Set::new(HashSet::<Value>::new()),
            nullifiers: Set::new(HashSet::<Value>::new()),
            gsrs: Array::new(Vec::<Value>::new()),
            public_objects: Dictionary::new(HashMap::new()),
        }
    }

    pub fn from_txs<T, FHash, FNullifiers>(
        block_number: i64,
        txs: &[T],
        prior_state_root_hashes: &[Hash],
        tx_hash: FHash,
        nullifier_hashes: FNullifiers,
    ) -> Self
    where
        FHash: Fn(&T) -> Hash,
        FNullifiers: Fn(&T) -> Vec<Hash>,
    {
        let transactions = Set::new(
            txs.iter()
                .map(|tx| Value::from(tx_hash(tx)))
                .collect::<HashSet<_>>(),
        );
        let nullifiers = Set::new(
            txs.iter()
                .flat_map(nullifier_hashes)
                .map(Value::from)
                .collect::<HashSet<_>>(),
        );
        let gsrs = Array::new(
            prior_state_root_hashes
                .iter()
                .copied()
                .map(Value::from)
                .collect(),
        );
        Self {
            block_number,
            transactions,
            nullifiers,
            gsrs,
            public_objects: Dictionary::new(HashMap::new()),
        }
    }

    pub fn roots(&self) -> (Hash, Hash, Hash, Hash) {
        (
            self.transactions.commitment(),
            self.nullifiers.commitment(),
            self.gsrs.commitment(),
            self.public_objects.commitment(),
        )
    }

    pub fn build_grounding_witness<T, W, FHash>(
        &self,
        source_txs: &[T],
        tx_hash: FHash,
        build: impl FnOnce(i64, Hash, Hash, Hash, Hash, HashMap<Hash, MerkleProof>) -> W,
    ) -> W
    where
        FHash: Fn(&T) -> Hash,
    {
        let source_tx_proofs = source_txs
            .iter()
            .map(|tx| {
                let tx_hash = tx_hash(tx);
                let proof = self
                    .transactions
                    .prove(&Value::from(tx_hash))
                    .expect("source tx should be provable from test state");
                (tx_hash, proof)
            })
            .collect::<HashMap<_, _>>();
        let (transactions_root, nullifiers_root, gsrs_root, public_objects_root) = self.roots();
        build(
            self.block_number,
            transactions_root,
            nullifiers_root,
            gsrs_root,
            public_objects_root,
            source_tx_proofs,
        )
    }

    pub fn tx_membership_proof(&self, tx_hash: Hash) -> MerkleProof {
        self.transactions
            .prove(&Value::from(tx_hash))
            .expect("tx should be provable from test state")
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

    pub fn apply_tx(&mut self, tx_hash: Hash, nullifier_hashes: impl IntoIterator<Item = Hash>) {
        self.transactions
            .insert(&Value::from(tx_hash))
            .expect("tx hash should insert into test state");
        for nullifier in nullifier_hashes {
            self.nullifiers
                .insert(&Value::from(nullifier))
                .expect("nullifier should insert into test state");
        }
        self.block_number += 1;
    }
}
