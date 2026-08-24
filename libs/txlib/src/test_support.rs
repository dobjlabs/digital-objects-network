//! Fixtures for exercising the transaction builder against the
//! `crafting_test` predicates.
//!
//! A plain public module rather than a `cfg(test)` one, so a crate
//! layered above this one can set a scenario up the same way its own
//! tests do. `MockProver` arrives through pod2, which this crate
//! already depends on unconditionally, so gating it would buy nothing.

use std::collections::HashMap;
use std::sync::Arc;

use hex::FromHex;
use pod2::{
    backends::plonky2::mock::mainpod::MockProver,
    frontend::{MainPod, MultiPodBuilder},
    middleware::{
        F, Hash, StrKey, Value,
        containers::{Array, Dictionary, Set},
    },
};
use pod2utils::{dict, rand_raw_value, set};

use crate::{GroundingWitness, StateHeader, Tx};

/// Running grounding state for the tests: keeps the full created-object set
/// (as an array, plus a reverse index for proofs) and the nullifier set so
/// it can hand out real Merkle proofs, while exposing only the
/// commitments-only `StateHeader`. The created set is grow-only.
pub struct TestState {
    block_number: i64,
    block_timestamp: i64,
    block_hash: Hash,
    created: Array,
    created_index: HashMap<Hash, i64>,
    nullifiers: Set,
    state_history: Array,
}

impl TestState {
    pub fn empty(block_number: i64) -> Self {
        Self {
            block_number,
            block_timestamp: block_number * 1000,
            block_hash: Hash([F(0), F(0), F(0), F(block_number as u64)]),
            created: Array::new(Vec::new()),
            created_index: HashMap::new(),
            nullifiers: set!(),
            state_history: Array::new(Vec::new()),
        }
    }

    pub fn state_header(&self) -> StateHeader {
        StateHeader::new(
            self.block_number,
            self.block_timestamp,
            self.block_hash,
            self.created.commitment(),
            self.nullifiers.commitment(),
            self.state_history.commitment(),
        )
    }

    pub fn apply_tx(&mut self, tx: &Tx) {
        for obj in tx.live.iter() {
            let obj = obj.expect("tx live entry should decode");
            let commitment = Hash(obj.raw().0);
            let index = self.created_index.len() as i64;
            self.created.insert(index as usize, obj).unwrap();
            self.created_index.insert(commitment, index);
        }
        for nullifier in tx.nullifiers.iter() {
            let nullifier = nullifier.expect("tx nullifier should decode");
            self.nullifiers.insert(&nullifier).unwrap();
        }
    }

    /// Build a grounding witness for the given input objects: one created-set
    /// `(index, membership proof)` per object, keyed by commitment.
    pub fn grounding_witness(&self, inputs: &[Dictionary]) -> Arc<GroundingWitness> {
        let created_proofs = inputs
            .iter()
            .map(|obj| {
                let commitment = obj.commitment();
                let index = *self
                    .created_index
                    .get(&commitment)
                    .expect("input object should be present in created set");
                let (_value, proof) = self
                    .created
                    .prove(index as usize)
                    .expect("input object should be provable from created set");
                (commitment, (index, proof))
            })
            .collect();
        Arc::new(GroundingWitness::new(self.state_header(), created_proofs))
    }
}

pub fn solve_and_verify(builder: MultiPodBuilder) -> MainPod {
    eprintln!("resource summary: {}", builder.resource_summary());
    let solution = builder.solve().unwrap();
    eprintln!("solution: {}", solution.solution_breakdown());
    let pod = solution.prove(&MockProver {}).unwrap().output_pod().clone();
    pod.pod.verify().unwrap();
    pod
}

pub fn make_object(guard_hash: Value, fields: &[(&str, Value)]) -> Dictionary {
    let mut d = dict!({
        "type" => guard_hash,
        "key" => rand_raw_value()
    });
    for (k, v) in fields {
        d.insert(&StrKey::from(*k), v).unwrap();
    }
    d
}

pub fn test_hash(byte: u8) -> Hash {
    Hash::from_hex(hex::encode([byte; 32])).expect("valid test hash")
}
