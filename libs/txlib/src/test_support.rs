//! Fixtures for exercising the transaction builder against the
//! `crafting_test` predicates, shared with the crates layered above
//! this one so a scenario is set up the same way on both sides of the
//! crate boundary.

use std::collections::HashMap;
use std::sync::Arc;

use hex::FromHex;
use pod2::{
    backends::plonky2::mock::mainpod::MockProver,
    frontend::{MainPod, MultiPodBuilder},
    lang::Module,
    middleware::{
        F, Hash, Params, Predicate, Statement, StrKey, VDSet, Value,
        containers::{Array, Dictionary, Set},
    },
};
use pod2utils::{dict, macros::BuildContext, map, op, rand_raw_value, set};

use crate::{GroundingWitness, StateHeader, Tx, TxBuilder};

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

/// The four modules a crafting test needs, plus the `IsWoodPick`
/// guard hash that objects of that class carry as their type.
pub fn craft_modules() -> (Vec<Arc<Module>>, Value) {
    let craft = Arc::new(crate::predicates::crafting_test_module());
    let is_wood_pick =
        Value::from(Predicate::Custom(craft.predicate_ref_by_name("IsWoodPick").unwrap()).hash());
    let modules = vec![
        Arc::new(crate::predicates::events_module()),
        Arc::new(crate::predicates::rekey_module()),
        Arc::new(crate::predicates::module()),
        craft,
    ];
    (modules, is_wood_pick)
}

/// Apply an ordinary single-party transaction that spawns a WoodPick,
/// and fold it into `state`. Returns the live pick.
pub fn spawn_wood_pick(
    state: &mut TestState,
    modules: &[Arc<Module>],
    is_wood_pick: Value,
) -> Dictionary {
    let mut ctx = BuildContext {
        builder: MultiPodBuilder::new(&Params::default(), &VDSet::new(&[])),
        modules: modules.to_vec(),
    };
    let initial = make_object(is_wood_pick, &[("durability", Value::from(100_i64))]);
    let mut tx = TxBuilder::new(&mut ctx, &[], state.grounding_witness(&[]));
    let scope = tx.begin_action();
    let (pick, st_insert, h) = tx.insert(&mut ctx, &initial);
    let op_dur = ctx
        .builder
        .priv_op(op!(DictContains(pick, "durability", 100_i64)))
        .unwrap();
    let st_spawn = ctx
        .apply_custom_pred_simple(false, "SpawnWoodPick", vec![op_dur, st_insert])
        .unwrap();
    tx.set_guard(h, is_wood_pick_guard(&mut ctx, state, 0, st_spawn));
    tx.end_action(scope);
    let (st, tx_out, _) = tx.finalize(&mut ctx);
    ctx.builder.reveal(&st).unwrap();
    solve_and_verify(ctx.builder);
    state.apply_tx(&tx_out);
    pick
}

/// Apply `IsWoodPick` with `st` in OR branch `branch`. Keeps the
/// guard's branch count in one place, since adding a branch would
/// otherwise widen the premise list at every call site.
pub fn is_wood_pick_guard(
    ctx: &mut BuildContext,
    state: &TestState,
    branch: usize,
    st: Statement,
) -> Statement {
    let mut premises = vec![Statement::None; 4];
    premises[branch] = st;
    ctx.apply_custom_pred(
        false,
        "IsWoodPick",
        map!({"state_header" => state.state_header().array()}),
        premises,
    )
    .unwrap()
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
