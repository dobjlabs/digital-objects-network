//! Tests for the transaction builder: end-to-end transactions proven
//! with `MockProver`, plus unit tests for the object derivations.
//!
//! `TestState` stands in for the synchronizer, keeping the full created
//! set so it can hand out real Merkle proofs for grounding.

use std::collections::HashMap;

use hex::FromHex;
use pod2::{
    backends::plonky2::mock::mainpod::MockProver,
    frontend::{MainPod, MultiPodBuilder},
    lang::Module,
    middleware::{F, Params, Predicate, VDSet, containers::Array},
};
use pod2utils::{macros::BuildContext, rand_raw_value, set};

use super::*;

/// Running grounding state for the tests: keeps the full created-object set
/// (as an array, plus a reverse index for proofs) and the nullifier set so
/// it can hand out real Merkle proofs, while exposing only the
/// commitments-only `StateHeader`. The created set is grow-only.
struct TestState {
    block_number: i64,
    block_timestamp: i64,
    block_hash: Hash,
    created: Array,
    created_index: HashMap<Hash, i64>,
    nullifiers: Set,
    state_history: Array,
}

impl TestState {
    fn empty(block_number: i64) -> Self {
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

    fn state_header(&self) -> StateHeader {
        StateHeader::new(
            self.block_number,
            self.block_timestamp,
            self.block_hash,
            self.created.commitment(),
            self.nullifiers.commitment(),
            self.state_history.commitment(),
        )
    }

    fn apply_tx(&mut self, tx: &Tx) {
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
    fn grounding_witness(&self, inputs: &[Dictionary]) -> Arc<GroundingWitness> {
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

fn solve_and_verify(builder: MultiPodBuilder) -> MainPod {
    eprintln!("resource summary: {}", builder.resource_summary());
    let solution = builder.solve().unwrap();
    eprintln!("solution: {}", solution.solution_breakdown());
    let pod = solution.prove(&MockProver {}).unwrap().output_pod().clone();
    pod.pod.verify().unwrap();
    pod
}

/// The four modules a crafting test needs, plus the `IsWoodPick`
/// guard hash that objects of that class carry as their type.
fn craft_modules() -> (Vec<Arc<Module>>, Value) {
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
fn spawn_wood_pick(
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
fn is_wood_pick_guard(
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

/// The plan both parties to a single transfer of `pick` derive
/// independently from the agreed effect: one action mutating `pick`
/// into the state `receiver_key` produces. The consumed side's
/// nullifier is the owner's contribution; everything else here is
/// computable from disclosed data. Header-free: `TxPlan::context`
/// brings the grounding header in.
fn transfer_plan(pick: &Dictionary, receiver_key: &Value) -> TxPlan {
    let projected = obj_with_key(&erased_key_state(pick), receiver_key.clone());
    TxPlan::new(
        vec![pick.commitment()],
        vec![PlannedEvent::Action(vec![PlannedEvent::Mutate {
            old: pick.commitment(),
            new: projected.commitment(),
            nullifier: compute_nullifier(pick),
        }])],
    )
    .expect("single-transfer plan is well formed")
}

/// Assert `plan`'s derivation agrees with what a builder actually
/// recorded: one chain position per leaf event in recording order,
/// then the finalized sets and commitment. The fold threads a single
/// chain, so pinning every leaf's end position (plus the seed, checked
/// at builder construction) pins every start position too.
fn assert_plan_agrees(plan: &TxPlan, leaf_positions: &[Hash], tx: &Tx) {
    assert_eq!(plan.leaf_count(), leaf_positions.len());
    for (index, position) in leaf_positions.iter().enumerate() {
        assert_eq!(plan.event_range(index).1, *position);
    }
    assert_eq!(plan.chain_end(), *leaf_positions.last().unwrap());
    assert_eq!(plan.live().commitment(), tx.live.commitment());
    assert_eq!(plan.nullifiers().commitment(), tx.nullifiers.commitment());
    assert_eq!(plan.tx_final(), tx.dict().commitment());
}

/// Two-party transfer with the roles of assembler and receiver split:
/// the *sender* assembles, so the mutation's `new` side belongs to a
/// counterparty and the sender only ever sees its commitment.
///
/// This is the direction a swap needs for one of its two legs, whichever
/// party assembles. It takes three proving sessions rather than two,
/// because the receiver must prove the `Rekey` action itself (its new key
/// is a private wildcard of that predicate) and doing so consumes the
/// sender's key-erasing statement, so the offer has to be a pod before
/// the receiver can build on it.
#[test]
fn sender_assembled_transfer_never_opens_the_received_state() {
    let (modules, is_wood_pick) = craft_modules();
    let mut state = TestState::empty(0);
    let pick = spawn_wood_pick(&mut state, &modules, is_wood_pick);
    let params = Params::default();
    let vd_set = VDSet::new(&[]);
    let build_ctx = || BuildContext {
        builder: MultiPodBuilder::new(&params, &vd_set),
        modules: modules.clone(),
    };

    // Negotiation: the sender discloses every non-key field, so the
    // receiver can reconstruct the erased-key state and project the state
    // its own key will produce. Both parties derive the chain positions
    // and the context from the agreed effect.
    let mid = erased_key_state(&pick);
    let receiver_key = Value::from(rand_raw_value());
    let projected = obj_with_key(&mid, receiver_key.clone());
    let plan = transfer_plan(&pick, &receiver_key);
    let (chain_start, chain_end) = plan.event_range(0);
    let context = plan.context(state.state_header().hash());

    // 1. Sender proves its offer into a pod.
    let mut sender = build_ctx();
    let offer = TransferOffer::prove(&mut sender, context, &pick);
    let offer_pod = solve_and_verify(sender.builder);

    // 2. Receiver imports it, proves the transfer action and its class
    //    guard, and reveals the guard. Its key never leaves this builder.
    let mut receiver = build_ctx();
    receiver.builder.add_pod(offer_pod).unwrap();
    let (acceptance, received) = TransferAcceptance::prove(
        &mut receiver,
        &offer,
        &mid,
        receiver_key.clone(),
        chain_start,
        chain_end,
    );
    let st_guard = is_wood_pick_guard(&mut receiver, &state, 3, acceptance.st_rekey.clone());
    receiver.builder.reveal(&st_guard).unwrap();
    let receiver_pod = solve_and_verify(receiver.builder);

    // 3. Sender assembles against the receiver's statements. It holds
    //    `old`, so it derives the nullifier and endorsement itself and
    //    needs no SpendAuthorization; it holds nothing at all of `new`.
    let mut assembler = build_ctx();
    assembler.builder.add_pod(receiver_pod).unwrap();
    let mut tx = TxBuilder::new(
        &mut assembler,
        std::slice::from_ref(&pick),
        state.grounding_witness(std::slice::from_ref(&pick)),
    );
    let scope = tx.begin_action();
    let h = tx.rekey_send(&mut assembler, &pick, &acceptance);
    tx.set_guard(h, st_guard);
    tx.end_action(scope);

    eprintln!("{tx}");
    let (st, tx_out, stats) = tx.finalize(&mut assembler);
    print_stats(&stats);
    assembler.builder.reveal(&st).unwrap();
    solve_and_verify(assembler.builder);

    // The sender's assembly landed the receiver's state without ever
    // holding it, and matches the effect it endorsed.
    assert_eq!(received.commitment(), projected.commitment());
    assert_eq!(
        context_commitment(state.state_header().hash(), tx_out.dict().commitment()),
        context
    );
    assert!(
        tx_out
            .live
            .contains(&Value::from(projected.commitment()))
            .unwrap()
    );
    assert!(
        tx_out
            .nullifiers
            .contains(&Value::from(compute_nullifier(&pick)))
            .unwrap()
    );
    // The received state is the planned one (asserted above), so the
    // positions the receiver proved against and the effect that landed
    // are the plan's.
    assert_eq!(plan.tx_final(), tx_out.dict().commitment());
}

/// The receiver derives the chain positions itself rather than being
/// told them, so a receiver that derives them wrongly must not be able to
/// hand over a contribution that looks valid.
///
/// It fails at its own record time: the chain step is a `Hash` whose
/// output is checked as the operation is built, so the bad position is
/// caught before any statement exists, let alone a pod. Nothing to
/// detect downstream.
#[test]
#[should_panic(expected = "invalid arguments to HashFromEntries")]
fn transfer_action_cannot_be_proven_at_the_wrong_chain_position() {
    let (modules, is_wood_pick) = craft_modules();
    let mut state = TestState::empty(0);
    let pick = spawn_wood_pick(&mut state, &modules, is_wood_pick);
    let build_ctx = || BuildContext {
        builder: MultiPodBuilder::new(&Params::default(), &VDSet::new(&[])),
        modules: modules.clone(),
    };

    let mid = erased_key_state(&pick);
    let receiver_key = Value::from(rand_raw_value());
    let plan = transfer_plan(&pick, &receiver_key);
    let (chain_start, _) = plan.event_range(0);

    let mut sender = build_ctx();
    let offer = TransferOffer::prove(
        &mut sender,
        plan.context(state.state_header().hash()),
        &pick,
    );
    let offer_pod = solve_and_verify(sender.builder);

    let mut receiver = build_ctx();
    receiver.builder.add_pod(offer_pod).unwrap();
    // Wrong end position: not the hash step this event produces.
    let _ = TransferAcceptance::prove(
        &mut receiver,
        &offer,
        &mid,
        receiver_key,
        chain_start,
        test_hash(0xEE),
    );
}

/// Drive a two-party transfer of `pick` to a receiver who assembles
/// the transaction, with the owner endorsing whichever `context` the
/// caller supplies.
///
/// Neither party learns the other's key: the owner needs only the
/// commitment of the state the receiver's key produces, and the
/// receiver only ever gets statements. Each runs its own builder and
/// the receiver imports the owner's pod.
fn run_two_party_transfer(
    state: &TestState,
    modules: &[Arc<Module>],
    pick: &Dictionary,
    receiver_key: Value,
    context: Hash,
) -> (Tx, Dictionary) {
    let params = Params::default();
    let vd_set = VDSet::new(&[]);

    // Owner's side: three statements, none revealing its key.
    let mut owner = BuildContext {
        builder: MultiPodBuilder::new(&params, &vd_set),
        modules: modules.to_vec(),
    };
    let offer = TransferOffer::prove(&mut owner, context, pick);
    let owner_pod = solve_and_verify(owner.builder);

    // Receiver assembles, grounding an input it holds only a
    // commitment for.
    let mut receiver = BuildContext {
        builder: MultiPodBuilder::new(&params, &vd_set),
        modules: modules.to_vec(),
    };
    receiver.builder.add_pod(owner_pod).unwrap();
    let mut tx = TxBuilder::new_from_commitments(
        &mut receiver,
        &[pick.commitment()],
        state.grounding_witness(std::slice::from_ref(pick)),
    );
    let scope = tx.begin_action();
    let mid = erased_key_state(pick);
    let (received, st_rekey, h) = tx.rekey_receive(&mut receiver, &offer, &mid, receiver_key);
    tx.set_guard(h, is_wood_pick_guard(&mut receiver, state, 3, st_rekey));
    tx.end_action(scope);

    eprintln!("{tx}");
    let (st, tx_out, stats) = tx.finalize(&mut receiver);
    print_stats(&stats);
    receiver.builder.reveal(&st).unwrap();
    solve_and_verify(receiver.builder);
    (tx_out, received)
}

/// End-to-end two-party transfer: the owner hands a WoodPick to a
/// receiver, who assembles and proves the transaction.
#[test]
fn two_party_rekey_transfers_without_sharing_keys() {
    let (modules, is_wood_pick) = craft_modules();
    let mut state = TestState::empty(0);
    let pick = spawn_wood_pick(&mut state, &modules, is_wood_pick);

    let receiver_key = Value::from(rand_raw_value());
    let context = transfer_plan(&pick, &receiver_key).context(state.state_header().hash());
    let (tx_out, received) =
        run_two_party_transfer(&state, &modules, &pick, receiver_key.clone(), context);

    // The receiver's assembly produced exactly the effect both
    // parties endorsed, so the owner's endorsement is valid for it.
    assert_eq!(
        received.get(&StrKey::from("key")).unwrap().unwrap(),
        receiver_key
    );
    assert_eq!(
        context_commitment(state.state_header().hash(), tx_out.dict().commitment()),
        context
    );
    assert!(
        tx_out
            .nullifiers
            .contains(&Value::from(compute_nullifier(&pick)))
            .unwrap()
    );
    assert!(
        tx_out
            .live
            .contains(&Value::from(received.commitment()))
            .unwrap()
    );
    // Only the key moved.
    assert_eq!(
        erased_key_state(&received).commitment(),
        erased_key_state(&pick).commitment()
    );
}

/// A swap: two top-level Rekey actions sharing one plan and one
/// context. Bob assembles, so his leg arrives via `rekey_receive` and
/// the leg he gives via `rekey_send`. The statement graph's schedule
/// is asserted first and then executed session by session: two
/// exchanges, three proving sessions, and only the erasure-before-
/// Rekey edge forces the sequentiality.
#[test]
fn swap_follows_the_two_exchange_schedule() {
    use graph::{NodeKind, StatementGraph, StatementNode};

    let (modules, is_wood_pick) = craft_modules();
    let mut state = TestState::empty(0);
    let pick_a = spawn_wood_pick(&mut state, &modules, is_wood_pick.clone());
    let pick_b = spawn_wood_pick(&mut state, &modules, is_wood_pick);
    let params = Params::default();
    let vd_set = VDSet::new(&[]);
    let build_ctx = || BuildContext {
        builder: MultiPodBuilder::new(&params, &vd_set),
        modules: modules.clone(),
    };

    // Data round: each side disclosed its pick's non-key fields; each
    // receiver projects the state its own (never disclosed) key
    // produces, and each owner contributes its spend's nullifier.
    let key_a = Value::from(rand_raw_value());
    let key_b = Value::from(rand_raw_value());
    let bob_projected = obj_with_key(&erased_key_state(&pick_a), key_b.clone());
    let alice_projected = obj_with_key(&erased_key_state(&pick_b), key_a.clone());
    let plan = TxPlan::new(
        vec![pick_a.commitment(), pick_b.commitment()],
        vec![
            PlannedEvent::Action(vec![PlannedEvent::Mutate {
                old: pick_a.commitment(),
                new: bob_projected.commitment(),
                nullifier: compute_nullifier(&pick_a),
            }]),
            PlannedEvent::Action(vec![PlannedEvent::Mutate {
                old: pick_b.commitment(),
                new: alice_projected.commitment(),
                nullifier: compute_nullifier(&pick_b),
            }]),
        ],
    )
    .unwrap();
    let context = plan.context(state.state_header().hash());

    // The statement graph. Alice's acceptance of Bob's leg is the one
    // node with a foreign premise besides the finalize.
    let graph = StatementGraph::new(vec![
        StatementNode::new(
            "offer:pick_a",
            "alice",
            NodeKind::TransferOffer {
                object: pick_a.commitment(),
            },
            &[],
        ),
        StatementNode::new(
            "offer:pick_b",
            "bob",
            NodeKind::TransferOffer {
                object: pick_b.commitment(),
            },
            &[],
        ),
        StatementNode::new(
            "accept:pick_b",
            "alice",
            NodeKind::TransferAcceptance {
                object: pick_b.commitment(),
            },
            &["offer:pick_b"],
        ),
        StatementNode::new(
            "finalize",
            "bob",
            NodeKind::Finalize,
            &["offer:pick_a", "accept:pick_b"],
        ),
    ])
    .unwrap();
    assert_eq!(
        graph.schedule().to_string(),
        "round 0: bob proves [offer:pick_b]\n\
         round 1: alice proves [offer:pick_a, accept:pick_b] importing [offer:pick_b]\n\
         round 2: bob proves [finalize] importing [offer:pick_a, accept:pick_b]\n"
    );

    // Round 0: Bob proves the offer for the leg he gives.
    let mut bob_offer_session = build_ctx();
    let offer_b = TransferOffer::prove(&mut bob_offer_session, context, &pick_b);
    let bob_offer_pod = solve_and_verify(bob_offer_session.builder);

    // Round 1: Alice proves everything of hers in one session: her own
    // offer, and her acceptance of Bob's leg at the plan positions that
    // account for leg 1 being recorded first.
    let mut alice_session = build_ctx();
    alice_session.builder.add_pod(bob_offer_pod).unwrap();
    let offer_a = TransferOffer::prove(&mut alice_session, context, &pick_a);
    let (leg2_prev, leg2_chain) = plan.event_range(1);
    let mid_b = erased_key_state(&pick_b);
    let (acceptance, alices_pick) = TransferAcceptance::prove(
        &mut alice_session,
        &offer_b,
        &mid_b,
        key_a,
        leg2_prev,
        leg2_chain,
    );
    let st_guard_leg2 =
        is_wood_pick_guard(&mut alice_session, &state, 3, acceptance.st_rekey.clone());
    alice_session.builder.reveal(&st_guard_leg2).unwrap();
    let alice_pod = solve_and_verify(alice_session.builder);

    // Round 2: Bob assembles both legs and finalizes.
    let mut bob = build_ctx();
    bob.builder.add_pod(alice_pod).unwrap();
    let mut tx = TxBuilder::new_from_commitments(
        &mut bob,
        &[pick_a.commitment(), pick_b.commitment()],
        state.grounding_witness(&[pick_a.clone(), pick_b.clone()]),
    );
    assert_eq!(plan.chain_start(), tx.chain_start);
    let mut leaf_positions = Vec::new();

    let scope = tx.begin_action();
    let mid_a = erased_key_state(&pick_a);
    let (bobs_pick, st_rekey, h) = tx.rekey_receive(&mut bob, &offer_a, &mid_a, key_b);
    leaf_positions.push(tx.chain_position());
    tx.set_guard(h, is_wood_pick_guard(&mut bob, &state, 3, st_rekey));
    tx.end_action(scope);

    let scope = tx.begin_action();
    let h = tx.rekey_send(&mut bob, &pick_b, &acceptance);
    leaf_positions.push(tx.chain_position());
    tx.set_guard(h, st_guard_leg2);
    tx.end_action(scope);

    eprintln!("{tx}");
    let (st, tx_out, stats) = tx.finalize(&mut bob);
    print_stats(&stats);
    bob.builder.reveal(&st).unwrap();
    solve_and_verify(bob.builder);

    // Each party controls the other's former pick: the received states
    // are the projections, which differ from the originals only in key.
    assert_eq!(bobs_pick.commitment(), bob_projected.commitment());
    assert_eq!(alices_pick.commitment(), alice_projected.commitment());
    assert_plan_agrees(&plan, &leaf_positions, &tx_out);
}

/// The endorsement has to bind. An `EndorseSpend` produced for a
/// different transaction context must not authorize this one, even
/// though it is a valid statement about the same object and carries
/// the right nullifier.
///
/// The assembler is honest here and simply cannot build the proof:
/// every other clause in the replay spine binds the real context, so
/// a stale endorsement leaves the `context` wildcard unsatisfiable.
#[test]
#[should_panic(expected = "context should be assigned the value")]
fn stale_context_endorsement_cannot_authorize_a_spend() {
    let (modules, is_wood_pick) = craft_modules();
    let mut state = TestState::empty(0);
    let pick = spawn_wood_pick(&mut state, &modules, is_wood_pick);

    let stale = context_commitment(state.state_header().hash(), test_hash(0xAB));
    run_two_party_transfer(
        &state,
        &modules,
        &pick,
        Value::from(rand_raw_value()),
        stale,
    );
}

fn make_object(guard_hash: Value, fields: &[(&str, Value)]) -> Dictionary {
    let mut d = dict!({
        "type" => guard_hash,
        "key" => rand_raw_value()
    });
    for (k, v) in fields {
        d.insert(&StrKey::from(*k), v).unwrap();
    }
    d
}

fn test_hash(byte: u8) -> Hash {
    Hash::from_hex(hex::encode([byte; 32])).expect("valid test hash")
}

#[test]
fn object_nullifier_hash_matches_key_hash_path() {
    let obj = new_obj();
    let key_hash = object_key_hash(&obj).unwrap();
    let nullifier = object_nullifier_hash(&obj).unwrap();
    assert_eq!(nullifier, object_nullifier_from_key_hash(key_hash));
    assert_eq!(nullifier, compute_nullifier(&obj));
}

#[test]
fn object_nullifier_hash_errors_without_key() {
    let mut obj = new_obj();
    obj.delete(&StrKey::from("key")).unwrap();
    let err = object_nullifier_hash(&obj).expect_err("missing key must fail");
    assert!(format!("{err}").contains("missing required key field"));
}

// The prover builds the context dict from full container values
// (header array, after_tx dict) so replay-time anchored-key rebinds
// can open it; the verifier rebuilds it from bare hashes
// (payload.state_root, payload.tx_final). Both must commit
// identically or no published proof would ever verify.
#[test]
fn context_commitment_matches_value_forms() {
    let sr = StateHeader::new(7, 8, test_hash(4), test_hash(1), test_hash(2), test_hash(3));
    let zero: Hash = EMPTY_VALUE.into();
    let tx_dict = build_tx(&set!(), &set!(), zero, zero);
    let full = dict!({
        "state_header" => sr.array(),
        "tx_commitment" => tx_dict.clone()
    });
    assert_eq!(
        full.commitment(),
        context_commitment(sr.hash(), tx_dict.commitment())
    );
}

#[test]
fn state_header_hash_matches_array_commitment() {
    let sr = StateHeader::new(7, 8, test_hash(4), test_hash(1), test_hash(2), test_hash(3));
    assert_eq!(sr.hash(), sr.array().commitment());
}

#[test]
fn state_header_serializes_and_deserializes_camelcase() {
    let original = StateHeader::new(
        9,
        10,
        test_hash(5),
        test_hash(1),
        test_hash(2),
        test_hash(3),
    );
    let encoded = serde_json::to_value(&original).unwrap();
    assert_eq!(encoded["blockNumber"], serde_json::json!(9));
    assert_eq!(encoded["blockTimestamp"], serde_json::json!(10));
    assert_eq!(
        encoded["blockHash"],
        serde_json::json!(hex::encode([5_u8; 32]))
    );
    assert_eq!(
        encoded["createdRoot"],
        serde_json::json!(hex::encode([1_u8; 32]))
    );
    assert_eq!(
        encoded["nullifiersRoot"],
        serde_json::json!(hex::encode([2_u8; 32]))
    );
    assert_eq!(
        encoded["priorStateHistoryRoot"],
        serde_json::json!(hex::encode([3_u8; 32]))
    );

    let decoded: StateHeader = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, original);
}

// TxPlan validates the same structure TxBuilder enforces at record
// time; a plan built from counterparty data must fail with a readable
// error rather than a panic.
#[test]
fn tx_plan_rejects_malformed_structures() {
    let a = test_hash(1);
    let b = test_hash(2);
    let n = test_hash(3);

    let err = TxPlan::new(vec![], vec![]).expect_err("empty plan");
    assert!(format!("{err}").contains("at least one action"));

    let err = TxPlan::new(vec![], vec![PlannedEvent::Insert { new: a }])
        .expect_err("bare top-level event");
    assert!(format!("{err}").contains("must be an action"));

    let err = TxPlan::new(vec![], vec![PlannedEvent::Action(vec![])]).expect_err("empty action");
    assert!(format!("{err}").contains("at least one event"));

    let spend_unknown = PlannedEvent::Action(vec![PlannedEvent::Mutate {
        old: a,
        new: b,
        nullifier: n,
    }]);
    let err = TxPlan::new(vec![], vec![spend_unknown]).expect_err("mutate of a non-live state");
    assert!(format!("{err}").contains("not live"));

    let double_insert = PlannedEvent::Action(vec![
        PlannedEvent::Insert { new: a },
        PlannedEvent::Insert { new: a },
    ]);
    let err = TxPlan::new(vec![], vec![double_insert]).expect_err("duplicate insert");
    assert!(format!("{err}").contains("duplicate created state"));

    let err = TxPlan::new(
        vec![a, a],
        vec![PlannedEvent::Action(vec![PlannedEvent::Insert { new: b }])],
    )
    .expect_err("duplicate input");
    assert!(format!("{err}").contains("duplicate input"));
}

// Scope algebra on a shape the proving scenarios never produce: a
// direct leaf on each side of a sub-action.
#[test]
fn tx_plan_scope_is_the_innermost_enclosing_action_range() {
    let plan = TxPlan::new(
        vec![],
        vec![PlannedEvent::Action(vec![
            PlannedEvent::Insert { new: test_hash(1) },
            PlannedEvent::Action(vec![PlannedEvent::Insert { new: test_hash(2) }]),
            PlannedEvent::Insert { new: test_hash(3) },
        ])],
    )
    .unwrap();

    assert_eq!(plan.leaf_count(), 3);
    // The chain folds flat: each leaf starts where the previous one
    // ended, actions contribute no steps of their own.
    assert_eq!(plan.event_range(0).0, plan.chain_start());
    assert_eq!(plan.event_range(1).0, plan.event_range(0).1);
    assert_eq!(plan.event_range(2).0, plan.event_range(1).1);
    assert_eq!(plan.chain_end(), plan.event_range(2).1);
    // Direct leaves of the outer action share its whole range, even
    // the one recorded before the sub-action closed.
    assert_eq!(plan.scope(0), (plan.chain_start(), plan.chain_end()));
    assert_eq!(plan.scope(0), plan.scope(2));
    // The nested leaf's scope is the sub-action's range only.
    assert_eq!(
        plan.scope(1),
        (plan.event_range(0).1, plan.event_range(1).1)
    );
}

#[test]
fn tx_plan_serde_round_trip() {
    let old = test_hash(1);
    let plan = TxPlan::new(
        vec![old],
        vec![PlannedEvent::Action(vec![
            PlannedEvent::Mutate {
                old,
                new: test_hash(2),
                nullifier: test_hash(3),
            },
            PlannedEvent::Insert { new: test_hash(4) },
        ])],
    )
    .unwrap();

    let encoded = serde_json::to_string(&plan).unwrap();
    let decoded: TxPlan = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.inputs(), plan.inputs());
    assert_eq!(decoded.events(), plan.events());
    assert_eq!(decoded.tx_final(), plan.tx_final());
    assert_eq!(decoded.event_range(0), plan.event_range(0));
    assert_eq!(decoded.scope(1), plan.scope(1));

    // A malformed document fails at deserialization, not at first use.
    let malformed = serde_json::json!({"inputs": [], "events": []});
    let err = serde_json::from_value::<TxPlan>(malformed).expect_err("empty plan document");
    assert!(format!("{err}").contains("at least one action"));
}

/// Tx 1: Spawn a WoodPick (insert, no inputs).
/// Tx 2: MineStone using the WoodPick (mutate pick + insert stone).
#[test]
fn test_mine_stone() {
    let events = Arc::new(crate::predicates::events_module());
    let txlib = Arc::new(crate::predicates::module());
    let craft = Arc::new(crate::predicates::crafting_test_module());
    let modules = vec![events, txlib.clone(), craft.clone()];

    let is_wood_pick =
        Value::from(Predicate::Custom(craft.predicate_ref_by_name("IsWoodPick").unwrap()).hash());
    let is_stone =
        Value::from(Predicate::Custom(craft.predicate_ref_by_name("IsStone").unwrap()).hash());

    let mut state = TestState::empty(0);
    let params = Params::default();
    let vd_set = VDSet::new(&[]);

    // ---- Tx 1: Spawn a WoodPick ----

    let builder = MultiPodBuilder::new(&params, &vd_set);
    let mut ctx = BuildContext {
        builder,
        modules: modules.clone(),
    };

    let pick_initial = make_object(
        is_wood_pick.clone(),
        &[("durability", Value::from(100_i64))],
    );

    let mut tx1 = TxBuilder::new(&mut ctx, &[], state.grounding_witness(&[]));

    let scope = tx1.begin_action();
    let (pick, st_insert, h) = tx1.insert(&mut ctx, &pick_initial);
    let op_dur = ctx
        .builder
        .priv_op(op!(DictContains(pick, "durability", 100_i64)))
        .unwrap();
    let st_spawn = ctx
        .apply_custom_pred_simple(false, "SpawnWoodPick", vec![op_dur, st_insert])
        .unwrap();
    let st_guard = ctx
        .apply_custom_pred(
            false,
            "IsWoodPick",
            map!({"state_header" => state.state_header().array()}),
            vec![
                st_spawn.clone(),
                Statement::None,
                Statement::None,
                Statement::None,
            ],
        )
        .unwrap();
    tx1.set_guard(h, st_guard);
    tx1.end_action(scope);

    eprintln!("{tx1}");
    let (st, tx0, stats) = tx1.finalize(&mut ctx);
    print_stats(&stats);
    ctx.builder.reveal(&st).unwrap();
    solve_and_verify(ctx.builder);

    state.apply_tx(&tx0);

    // ---- Tx 2: MineStone ----

    let builder = MultiPodBuilder::new(&params, &vd_set);
    let mut ctx = BuildContext { builder, modules };

    let mut pick_new = pick.clone();
    pick_new
        .update(&StrKey::from("durability"), &Value::from(99_i64))
        .unwrap();
    let stone_initial = make_object(is_stone.clone(), &[]);

    // The plan mirrors the event tree recorded below; every derived
    // position and quantity must agree with the builder's.
    let plan = TxPlan::new(
        vec![pick.commitment()],
        vec![PlannedEvent::Action(vec![
            PlannedEvent::Action(vec![PlannedEvent::Mutate {
                old: pick.commitment(),
                new: pick_new.commitment(),
                nullifier: compute_nullifier(&pick),
            }]),
            PlannedEvent::Insert {
                new: with_stable_identifier(&stone_initial).commitment(),
            },
        ])],
    )
    .unwrap();

    let inputs = vec![pick.clone()];
    let witness = state.grounding_witness(&inputs);
    let mut tx2 = TxBuilder::new(&mut ctx, &inputs, witness);
    assert_eq!(plan.chain_start(), tx2.chain_start);
    let mut leaf_positions = Vec::new();

    let scope_outer = tx2.begin_action();

    // Sub-action: UseWoodPick (mutate pick)
    let st_use_wp = {
        let scope_sub = tx2.begin_action();
        let (st_mutate, h_sub) = tx2.mutate(&mut ctx, &pick_new, &pick);
        leaf_positions.push(tx2.chain_position());
        let op_gt = ctx
            .builder
            .priv_op(op!(Gt((&pick, "durability"), 0_i64)))
            .unwrap();
        let op_sum = ctx
            .builder
            .priv_op(op!(Sum(99_i64, 1_i64, (&pick, "durability"))))
            .unwrap();
        let op_du = ctx
            .builder
            .priv_op(op!(DictUpdate(pick, "durability", 99_i64, pick_new)))
            .unwrap();
        let st_action = ctx
            .apply_custom_pred_simple(false, "UseWoodPick", vec![op_gt, op_sum, op_du, st_mutate])
            .unwrap();
        let st_guard = ctx
            .apply_custom_pred(
                false,
                "IsWoodPick",
                map!({"state_header" => state.state_header().array()}),
                vec![
                    Statement::None,
                    Statement::None,
                    st_action.clone(),
                    Statement::None,
                ],
            )
            .unwrap();
        tx2.set_guard(h_sub, st_guard);
        tx2.end_action(scope_sub);
        assert_eq!(plan.scope(0), (plan.chain_start(), tx2.chain_position()));
        st_action
    };

    // Direct: insert stone
    let (_stone, st_stone_insert, h) = tx2.insert(&mut ctx, &stone_initial);
    leaf_positions.push(tx2.chain_position());
    let st_mine = ctx
        .apply_custom_pred_simple(false, "MineStone", vec![st_use_wp, st_stone_insert])
        .unwrap();
    let st_guard = ctx
        .apply_custom_pred(
            false,
            "IsStone",
            map!({"state_header" => state.state_header().array()}),
            vec![st_mine.clone()],
        )
        .unwrap();
    tx2.set_guard(h, st_guard);
    tx2.end_action(scope_outer);

    eprintln!("{tx2}");
    let (st, tx_out, stats) = tx2.finalize(&mut ctx);
    print_stats(&stats);
    ctx.builder.reveal(&st).unwrap();
    solve_and_verify(ctx.builder);

    assert!(
        tx_out
            .nullifiers
            .contains(&Value::from(compute_nullifier(&pick)))
            .unwrap()
    );
    assert_plan_agrees(&plan, &leaf_positions, &tx_out);
}

/// Tx 1: FindLog (genesis insert).
/// Tx 2: CraftWood (delete log, insert wood).
/// Tx 3: CraftSticks (delete wood, insert two sticks).
#[test]
fn test_craft_sticks() {
    let events = Arc::new(crate::predicates::events_module());
    let txlib = Arc::new(crate::predicates::module());
    let craft = Arc::new(crate::predicates::crafting_test_module());
    let modules = vec![events, txlib.clone(), craft.clone()];

    let is_log =
        Value::from(Predicate::Custom(craft.predicate_ref_by_name("IsLog").unwrap()).hash());
    let is_wood =
        Value::from(Predicate::Custom(craft.predicate_ref_by_name("IsWood").unwrap()).hash());
    let is_stick =
        Value::from(Predicate::Custom(craft.predicate_ref_by_name("IsStick").unwrap()).hash());

    let mut state = TestState::empty(0);
    let params = Params::default();
    let vd_set = VDSet::new(&[]);

    // ---- Tx 1: FindLog ----

    let builder = MultiPodBuilder::new(&params, &vd_set);
    let mut ctx = BuildContext {
        builder,
        modules: modules.clone(),
    };

    let log_initial = make_object(is_log.clone(), &[]);

    let mut tx1 = TxBuilder::new(&mut ctx, &[], state.grounding_witness(&[]));

    let scope = tx1.begin_action();
    let (log, st_insert, h) = tx1.insert(&mut ctx, &log_initial);
    let st_find = ctx
        .apply_custom_pred_simple(false, "FindLog", vec![st_insert])
        .unwrap();
    let st_guard = ctx
        .apply_custom_pred(
            false,
            "IsLog",
            map!({"state_header" => state.state_header().array()}),
            vec![st_find.clone(), Statement::None],
        )
        .unwrap();
    tx1.set_guard(h, st_guard);
    tx1.end_action(scope);

    eprintln!("{tx1}");
    let (st, tx1_out, stats) = tx1.finalize(&mut ctx);
    print_stats(&stats);
    ctx.builder.reveal(&st).unwrap();
    solve_and_verify(ctx.builder);

    state.apply_tx(&tx1_out);

    // ---- Tx 2: CraftWood ----

    let builder = MultiPodBuilder::new(&params, &vd_set);
    let mut ctx = BuildContext {
        builder,
        modules: modules.clone(),
    };

    let wood_initial = make_object(is_wood.clone(), &[]);

    let inputs = vec![log.clone()];
    let witness = state.grounding_witness(&inputs);
    let mut tx2 = TxBuilder::new(&mut ctx, &inputs, witness);

    let scope_outer = tx2.begin_action();

    // Sub-action: DeleteLog
    let st_del_log = {
        let scope_sub = tx2.begin_action();
        let (st_del, h_sub) = tx2.delete(&mut ctx, &log);
        let st_action = ctx
            .apply_custom_pred_simple(false, "DeleteLog", vec![st_del])
            .unwrap();
        let st_guard = ctx
            .apply_custom_pred(
                false,
                "IsLog",
                map!({"state_header" => state.state_header().array()}),
                vec![Statement::None, st_action.clone()],
            )
            .unwrap();
        tx2.set_guard(h_sub, st_guard);
        tx2.end_action(scope_sub);
        st_action
    };

    // Direct: insert wood
    let (wood, st_ins, h) = tx2.insert(&mut ctx, &wood_initial);
    let st_craft_wood = ctx
        .apply_custom_pred_simple(false, "CraftWood", vec![st_del_log, st_ins])
        .unwrap();
    let st_guard = ctx
        .apply_custom_pred(
            false,
            "IsWood",
            map!({"state_header" => state.state_header().array()}),
            vec![st_craft_wood.clone(), Statement::None],
        )
        .unwrap();
    tx2.set_guard(h, st_guard);
    tx2.end_action(scope_outer);

    eprintln!("{tx2}");
    let (st, tx2_out, stats) = tx2.finalize(&mut ctx);
    print_stats(&stats);
    ctx.builder.reveal(&st).unwrap();
    solve_and_verify(ctx.builder);

    state.apply_tx(&tx2_out);

    // ---- Tx 3: CraftSticks ----

    let builder = MultiPodBuilder::new(&params, &vd_set);
    let mut ctx = BuildContext { builder, modules };

    let stick_a_initial = make_object(is_stick.clone(), &[]);
    let stick_b_initial = make_object(is_stick, &[]);

    // Plan mirror for the delete + two-inserts shape.
    let plan = TxPlan::new(
        vec![wood.commitment()],
        vec![PlannedEvent::Action(vec![
            PlannedEvent::Action(vec![PlannedEvent::Delete {
                old: wood.commitment(),
                nullifier: compute_nullifier(&wood),
            }]),
            PlannedEvent::Insert {
                new: with_stable_identifier(&stick_a_initial).commitment(),
            },
            PlannedEvent::Insert {
                new: with_stable_identifier(&stick_b_initial).commitment(),
            },
        ])],
    )
    .unwrap();

    let inputs = vec![wood.clone()];
    let witness = state.grounding_witness(&inputs);
    let mut tx3 = TxBuilder::new(&mut ctx, &inputs, witness);
    assert_eq!(plan.chain_start(), tx3.chain_start);
    let mut leaf_positions = Vec::new();

    let scope_outer = tx3.begin_action();

    // Sub-action: DeleteWood
    let st_del_wood = {
        let scope_sub = tx3.begin_action();
        let (st_del, h_sub) = tx3.delete(&mut ctx, &wood);
        leaf_positions.push(tx3.chain_position());
        let st_action = ctx
            .apply_custom_pred_simple(false, "DeleteWood", vec![st_del])
            .unwrap();
        let st_guard = ctx
            .apply_custom_pred(
                false,
                "IsWood",
                map!({"state_header" => state.state_header().array()}),
                vec![Statement::None, st_action.clone()],
            )
            .unwrap();
        tx3.set_guard(h_sub, st_guard);
        tx3.end_action(scope_sub);
        st_action
    };

    // Direct: insert stick_a
    let (stick_a, st_ins_a, h_a) = tx3.insert(&mut ctx, &stick_a_initial);
    leaf_positions.push(tx3.chain_position());

    // Direct: insert stick_b
    let (stick_b, st_ins_b, h_b) = tx3.insert(&mut ctx, &stick_b_initial);
    leaf_positions.push(tx3.chain_position());

    // Pack stick_a / stick_b's pre-identity initials into an
    // `initials` dict so CraftSticks stays within the 8-wildcard
    // limit; rebind each TxInsert's slot 2 (initial) onto the
    // matching anchored key. TxInsert's arg layout is (chain,
    // prev_chain, initial, new, type).
    let initials = dict!({
        "stick_a" => stick_a_initial.clone(),
        "stick_b" => stick_b_initial.clone()
    });
    let st_ins_a_anchored = ctx
        .builder
        .priv_op(Operation::replace_value_with_entry(
            vec![None, None, Some((&initials, "stick_a")), None, None],
            st_ins_a,
        ))
        .unwrap();
    let st_ins_b_anchored = ctx
        .builder
        .priv_op(Operation::replace_value_with_entry(
            vec![None, None, Some((&initials, "stick_b")), None, None],
            st_ins_b,
        ))
        .unwrap();
    let st_craft_sticks = ctx
        .apply_custom_pred_simple(
            false,
            "CraftSticks",
            vec![st_del_wood, st_ins_a_anchored, st_ins_b_anchored],
        )
        .unwrap();

    // stick_a: IsStick branch 2 = CraftSticks(obj, other, chain_start, chain_end)
    let st_is_stick_a = ctx
        .apply_custom_pred(
            false,
            "IsStick",
            map!({"state_header" => state.state_header().array()}),
            vec![Statement::None, st_craft_sticks.clone(), Statement::None],
        )
        .unwrap();
    tx3.set_guard(h_a, st_is_stick_a);

    // stick_b: IsStick branch 3 = CraftSticks(other, obj, chain_start, chain_end)
    let st_is_stick_b = ctx
        .apply_custom_pred(
            false,
            "IsStick",
            map!({"state_header" => state.state_header().array()}),
            vec![Statement::None, Statement::None, st_craft_sticks.clone()],
        )
        .unwrap();
    tx3.set_guard(h_b, st_is_stick_b);

    tx3.end_action(scope_outer);

    eprintln!("{tx3}");
    let (st, tx3_out, stats) = tx3.finalize(&mut ctx);
    print_stats(&stats);
    ctx.builder.reveal(&st).unwrap();
    solve_and_verify(ctx.builder);

    // Both sticks should be live
    assert!(tx3_out.live.contains(&Value::from(stick_a)).unwrap());
    assert!(tx3_out.live.contains(&Value::from(stick_b)).unwrap());
    // Wood should be nullified
    assert!(
        tx3_out
            .nullifiers
            .contains(&Value::from(compute_nullifier(&wood)))
            .unwrap()
    );
    assert_plan_agrees(&plan, &leaf_positions, &tx3_out);
}

/// Grounding three inputs exercises InputsGroundedRecursive (peel two per
/// level) bottoming out at InputsGroundedSingle -- the N>=3 path that the
/// one- and two-input tests never reach.
#[test]
fn test_grounds_three_inputs() {
    let events = Arc::new(crate::predicates::events_module());
    let txlib = Arc::new(crate::predicates::module());
    let craft = Arc::new(crate::predicates::crafting_test_module());
    let modules = vec![events, txlib.clone(), craft.clone()];

    let is_log =
        Value::from(Predicate::Custom(craft.predicate_ref_by_name("IsLog").unwrap()).hash());

    let mut state = TestState::empty(0);
    let params = Params::default();
    let vd_set = VDSet::new(&[]);

    // Spawn three logs (one FindLog tx each) and fold them into the live
    // set so the burn tx below can ground all three.
    let mut logs = Vec::new();
    for _ in 0..3 {
        let builder = MultiPodBuilder::new(&params, &vd_set);
        let mut ctx = BuildContext {
            builder,
            modules: modules.clone(),
        };
        let log_initial = make_object(is_log.clone(), &[]);
        let mut tx = TxBuilder::new(&mut ctx, &[], state.grounding_witness(&[]));
        let scope = tx.begin_action();
        let (log, st_insert, h) = tx.insert(&mut ctx, &log_initial);
        let st_find = ctx
            .apply_custom_pred_simple(false, "FindLog", vec![st_insert])
            .unwrap();
        let st_guard = ctx
            .apply_custom_pred(
                false,
                "IsLog",
                map!({"state_header" => state.state_header().array()}),
                vec![st_find, Statement::None],
            )
            .unwrap();
        tx.set_guard(h, st_guard);
        tx.end_action(scope);
        let (st, tx_out, _stats) = tx.finalize(&mut ctx);
        ctx.builder.reveal(&st).unwrap();
        solve_and_verify(ctx.builder);
        state.apply_tx(&tx_out);
        logs.push(log);
    }

    // Burn all three logs in one tx: TxBuilder::new grounds three inputs,
    // driving InputsGrounded -> Recursive -> InputsGrounded -> Single.
    let builder = MultiPodBuilder::new(&params, &vd_set);
    let mut ctx = BuildContext { builder, modules };

    let inputs = logs.clone();
    let witness = state.grounding_witness(&inputs);
    let mut burn = TxBuilder::new(&mut ctx, &inputs, witness);

    for log in &logs {
        let scope = burn.begin_action();
        let (st_del, h) = burn.delete(&mut ctx, log);
        let st_action = ctx
            .apply_custom_pred_simple(false, "DeleteLog", vec![st_del])
            .unwrap();
        let st_guard = ctx
            .apply_custom_pred(
                false,
                "IsLog",
                map!({"state_header" => state.state_header().array()}),
                vec![Statement::None, st_action],
            )
            .unwrap();
        burn.set_guard(h, st_guard);
        burn.end_action(scope);
    }

    eprintln!("{burn}");
    let (st, burn_out, stats) = burn.finalize(&mut ctx);
    print_stats(&stats);
    ctx.builder.reveal(&st).unwrap();
    solve_and_verify(ctx.builder);

    for log in &logs {
        assert!(
            burn_out
                .nullifiers
                .contains(&Value::from(compute_nullifier(log)))
                .unwrap()
        );
    }
}

/// Single-party transfer: spawn a WoodPick, then move it to a new
/// key via the `Rekey` branch of `IsWoodPick`. Checks that the
/// transfer spends the old state, leaves the new one live, and
/// preserves every field except the key.
#[test]
fn test_rekey_transfers_control() {
    let (modules, is_wood_pick) = craft_modules();
    let mut state = TestState::empty(0);
    let pick = spawn_wood_pick(&mut state, &modules, is_wood_pick);

    let mut ctx = BuildContext {
        builder: MultiPodBuilder::new(&Params::default(), &VDSet::new(&[])),
        modules,
    };
    let inputs = vec![pick.clone()];
    let witness = state.grounding_witness(&inputs);
    let mut tx = TxBuilder::new(&mut ctx, &inputs, witness);

    let receiver_key = Value::from(rand_raw_value());
    let scope = tx.begin_action();
    let (moved, st_rekey, h) = tx.rekey(&mut ctx, &pick, receiver_key.clone());
    tx.set_guard(h, is_wood_pick_guard(&mut ctx, &state, 3, st_rekey));
    tx.end_action(scope);

    eprintln!("{tx}");
    let (st, tx_out, stats) = tx.finalize(&mut ctx);
    print_stats(&stats);
    ctx.builder.reveal(&st).unwrap();
    solve_and_verify(ctx.builder);

    // The transferred state is live, the old one is spent.
    assert!(tx_out.live.contains(&Value::from(moved.clone())).unwrap());
    assert!(
        !tx_out
            .live
            .contains(&Value::from(pick.commitment()))
            .unwrap()
    );
    assert!(
        tx_out
            .nullifiers
            .contains(&Value::from(compute_nullifier(&pick)))
            .unwrap()
    );

    // Only the key moved: the new state is the old one with the
    // receiver's key, so both erase to the same intermediate.
    assert_eq!(
        moved.get(&StrKey::from("key")).unwrap().unwrap(),
        receiver_key
    );
    assert_eq!(
        erased_key_state(&moved).commitment(),
        erased_key_state(&pick).commitment()
    );
    assert_eq!(
        object_stable_identifier(&moved),
        object_stable_identifier(&pick)
    );

    // The transferred state's nullifier is the receiver's to produce:
    // it is keyed on the new key, not the old one.
    assert_ne!(compute_nullifier(&moved), compute_nullifier(&pick));
}
