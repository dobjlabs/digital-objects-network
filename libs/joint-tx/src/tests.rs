//! Joint-transaction scenarios and unit tests for plans, graphs, custody,
//! serialization, and contribution validation.
//! Cross-party artifacts are serialized and validated between sessions.

use std::sync::Arc;

use pod2::{
    frontend::MultiPodBuilder,
    lang::Module,
    middleware::{
        Hash, Params, Predicate, Statement, StrKey, VDSet, Value, containers::Dictionary,
    },
};
use pod2utils::{macros::BuildContext, map, op, rand_raw_value, set};
use txlib::{
    Tx, TxBuilder, chain_seed, chain_step, compute_nullifier, context_commitment, erased_key_state,
    event_hash_delete, obj_with_key, print_stats,
    test_support::{
        TestState, craft_modules, is_wood_pick_guard, make_object, solve_and_verify,
        spawn_wood_pick, test_hash,
    },
    top_level_tx, with_stable_identifier,
};

use crate::{
    InputState, JointEvent, JointTransaction, KeyAnnotation, PlannedEvent, TransferAcceptance,
    TransferOffer, TxPlan,
    graph::{NodeKind, StatementGraph},
};

/// Cross a simulated session boundary through wire encoding.
fn wire<T: serde::Serialize + serde::de::DeserializeOwned>(value: T) -> T {
    serde_json::from_slice(&serde_json::to_vec(&value).unwrap()).unwrap()
}
/// Derive the agreed one-leg transfer plan from disclosed data.
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

/// Check a builder's recorded positions and final effect against `plan`.
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

/// Build a `JointTransaction` from borrowed test data.
fn joint(
    participants: &[&str],
    finalizer: &str,
    inputs: &[(Hash, &str)],
    events: Vec<JointEvent>,
) -> anyhow::Result<JointTransaction> {
    JointTransaction::new(
        participants.iter().map(|p| p.to_string()).collect(),
        finalizer.to_string(),
        inputs
            .iter()
            .map(|(commitment, holder)| InputState {
                commitment: *commitment,
                holder: holder.to_string(),
            })
            .collect(),
        events,
    )
}

/// Sender-assembled transfer where the new state remains with the receiver.
/// The receiver imports the sender's offer before proving Rekey, so this
/// direction requires three proving sessions.
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

    // Disclosed non-key fields let the receiver reconstruct the erased state.
    let mid = erased_key_state(&pick);
    let receiver_key = Value::from(rand_raw_value());
    let projected = obj_with_key(&mid, receiver_key.clone());
    let plan = transfer_plan(&pick, &receiver_key);
    let (chain_start, chain_end) = plan.event_range(0);
    let context = plan.context(state.state_header().hash());

    let mut sender = build_ctx();
    let offer = TransferOffer::prove(&mut sender, context, &pick);
    let offer_pod = solve_and_verify(sender.builder);

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

    let mut assembler = build_ctx();
    assembler.builder.add_pod(receiver_pod).unwrap();
    let mut tx = TxBuilder::new(
        &mut assembler,
        std::slice::from_ref(&pick),
        state.grounding_witness(std::slice::from_ref(&pick)),
    );
    let scope = tx.begin_action();
    let h = tx.rekey_record(&mut assembler, &pick, &acceptance.state_facts());
    tx.set_guard(h, st_guard);
    tx.end_action(scope);

    eprintln!("{tx}");
    let (st, tx_out, stats) = tx.finalize(&mut assembler);
    print_stats(&stats);
    assembler.builder.reveal(&st).unwrap();
    solve_and_verify(assembler.builder);

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
    assert_eq!(plan.tx_final(), tx_out.dict().commitment());
}

/// Reject a transfer proved against an incorrect derived chain position.
/// The hash operation fails before a contribution can be exported.
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
    let _ = TransferAcceptance::prove(
        &mut receiver,
        &offer,
        &mid,
        receiver_key,
        chain_start,
        test_hash(0xEE),
    );
}

/// Run a receiver-assembled transfer with an endorsement for `context`.
fn run_two_party_transfer(
    state: &TestState,
    modules: &[Arc<Module>],
    pick: &Dictionary,
    receiver_key: Value,
    context: Hash,
) -> (Tx, Dictionary) {
    let params = Params::default();
    let vd_set = VDSet::new(&[]);

    let mut owner = BuildContext {
        builder: MultiPodBuilder::new(&params, &vd_set),
        modules: modules.to_vec(),
    };
    let offer = TransferOffer::prove(&mut owner, context, pick);
    let owner_pod = solve_and_verify(owner.builder);

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
    let (received, st_rekey, h) = tx.rekey_apply(
        &mut receiver,
        &offer.spend_facts(),
        offer.st_key_erasure.clone(),
        &mid,
        receiver_key,
    );
    tx.set_guard(h, is_wood_pick_guard(&mut receiver, state, 3, st_rekey));
    tx.end_action(scope);

    eprintln!("{tx}");
    let (st, tx_out, stats) = tx.finalize(&mut receiver);
    print_stats(&stats);
    receiver.builder.reveal(&st).unwrap();
    solve_and_verify(receiver.builder);
    (tx_out, received)
}

/// Transfer a WoodPick to a receiver that assembles the transaction.
#[test]
fn two_party_rekey_transfers_without_sharing_keys() {
    let (modules, is_wood_pick) = craft_modules();
    let mut state = TestState::empty(0);
    let pick = spawn_wood_pick(&mut state, &modules, is_wood_pick);

    let receiver_key = Value::from(rand_raw_value());
    let context = transfer_plan(&pick, &receiver_key).context(state.state_header().hash());
    let (tx_out, received) =
        run_two_party_transfer(&state, &modules, &pick, receiver_key.clone(), context);

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
    assert_eq!(
        erased_key_state(&received).commitment(),
        erased_key_state(&pick).commitment()
    );
}

/// Swap two states using one plan, one context, and a derived three-session
/// schedule. Every cross-party artifact crosses a validated wire boundary.
#[test]
fn swap_follows_the_two_exchange_schedule() {
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

    // Each receiver projects a new state from disclosed non-key fields.
    let key_a = Value::from(rand_raw_value());
    let key_b = Value::from(rand_raw_value());
    let bob_projected = obj_with_key(&erased_key_state(&pick_a), key_b.clone());
    let alice_projected = obj_with_key(&erased_key_state(&pick_b), key_a.clone());
    let jt = joint(
        &["alice", "bob"],
        "bob",
        &[(pick_a.commitment(), "alice"), (pick_b.commitment(), "bob")],
        vec![
            JointEvent::transfer_leg(
                "alice",
                "bob",
                pick_a.commitment(),
                bob_projected.commitment(),
                compute_nullifier(&pick_a),
            ),
            JointEvent::transfer_leg(
                "bob",
                "alice",
                pick_b.commitment(),
                alice_projected.commitment(),
                compute_nullifier(&pick_b),
            ),
        ],
    )
    .unwrap();
    let plan = jt.plan();
    let context = plan.context(state.state_header().hash());

    // Alice's acceptance is the only non-finalize node with an import.
    assert_eq!(
        jt.graph().schedule().to_string(),
        "round 0: bob proves [offer:1]\n\
         round 1: alice proves [offer:0, accept:1] importing [offer:1]\n\
         round 2: bob proves [finalize] importing [offer:0, accept:1]\n"
    );

    let mut bob_offer_session = build_ctx();
    let offer_b = TransferOffer::prove(&mut bob_offer_session, context, &pick_b);
    let bob_offer_pod = solve_and_verify(bob_offer_session.builder);

    let (offer_b, bob_offer_pod) = wire((offer_b, bob_offer_pod));
    bob_offer_pod.pod.verify().unwrap();
    offer_b
        .validate(&bob_offer_pod, context, pick_b.commitment())
        .unwrap();

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

    let (offer_a, acceptance, st_guard_leg2, alice_pod) =
        wire((offer_a, acceptance, st_guard_leg2, alice_pod));
    alice_pod.pod.verify().unwrap();
    offer_a
        .validate(&alice_pod, context, pick_a.commitment())
        .unwrap();
    acceptance
        .validate(&alice_pod, alice_projected.commitment())
        .unwrap();
    acceptance
        .validate_guard(&alice_pod, &st_guard_leg2)
        .unwrap();

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
    let (bobs_pick, st_rekey, h) = tx.rekey_apply(
        &mut bob,
        &offer_a.spend_facts(),
        offer_a.st_key_erasure.clone(),
        &mid_a,
        key_b,
    );
    leaf_positions.push(tx.chain_position());
    tx.set_guard(h, is_wood_pick_guard(&mut bob, &state, 3, st_rekey));
    tx.end_action(scope);

    let scope = tx.begin_action();
    let h = tx.rekey_record(&mut bob, &pick_b, &acceptance.state_facts());
    leaf_positions.push(tx.chain_position());
    tx.set_guard(h, st_guard_leg2);
    tx.end_action(scope);

    eprintln!("{tx}");
    let (st, tx_out, stats) = tx.finalize(&mut bob);
    print_stats(&stats);
    bob.builder.reveal(&st).unwrap();
    solve_and_verify(bob.builder);

    assert_eq!(bobs_pick.commitment(), bob_projected.commitment());
    assert_eq!(alices_pick.commitment(), alice_projected.commitment());
    assert_eq!(jt.final_custody()[&bob_projected.commitment()], "bob");
    assert_eq!(jt.final_custody()[&alice_projected.commitment()], "alice");
    assert_plan_agrees(plan, &leaf_positions, &tx_out);
}

/// Lend, use, and return a pick atomically under one agreed plan.
///
/// Bob finalizes because he records the internal MineStone action. That
/// action creates no graph node, so the transfer schedule matches the swap.
#[test]
fn borrow_act_return_follows_the_two_exchange_schedule() {
    let (modules, is_wood_pick) = craft_modules();
    let craft = Arc::new(txlib::predicates::crafting_test_module());
    let is_stone =
        Value::from(Predicate::Custom(craft.predicate_ref_by_name("IsStone").unwrap()).hash());
    let mut state = TestState::empty(0);
    let pick = spawn_wood_pick(&mut state, &modules, is_wood_pick);
    let params = Params::default();
    let vd_set = VDSet::new(&[]);
    let build_ctx = || BuildContext {
        builder: MultiPodBuilder::new(&params, &vd_set),
        modules: modules.clone(),
    };

    // The plan fixes the worn pick and minted stone before proving begins.
    let key_bob = Value::from(rand_raw_value());
    let key_alice = Value::from(rand_raw_value());
    let pick_lent = obj_with_key(&erased_key_state(&pick), key_bob.clone());
    let mut pick_used = pick_lent.clone();
    pick_used
        .update(&StrKey::from("durability"), &Value::from(99_i64))
        .unwrap();
    let stone_initial = make_object(is_stone, &[]);
    let stone = with_stable_identifier(&stone_initial);
    let pick_returned = obj_with_key(&erased_key_state(&pick_used), key_alice.clone());

    let jt = joint(
        &["alice", "bob"],
        "bob",
        &[(pick.commitment(), "alice")],
        vec![
            JointEvent::transfer_leg(
                "alice",
                "bob",
                pick.commitment(),
                pick_lent.commitment(),
                compute_nullifier(&pick),
            ),
            JointEvent::Action(vec![
                JointEvent::Action(vec![JointEvent::Mutate {
                    performer: "bob".into(),
                    old: pick_lent.commitment(),
                    new: pick_used.commitment(),
                    nullifier: compute_nullifier(&pick_lent),
                    key: KeyAnnotation::Inherited,
                }]),
                JointEvent::Insert {
                    minter: "bob".into(),
                    new: stone.commitment(),
                },
            ]),
            JointEvent::transfer_leg(
                "bob",
                "alice",
                pick_used.commitment(),
                pick_returned.commitment(),
                compute_nullifier(&pick_used),
            ),
        ],
    )
    .unwrap();
    let plan = jt.plan();
    let context = plan.context(state.state_header().hash());

    // MineStone is finalizer-internal and therefore absent from the graph.
    assert_eq!(
        jt.graph().schedule().to_string(),
        "round 0: bob proves [offer:2]\n\
         round 1: alice proves [offer:0, accept:2] importing [offer:2]\n\
         round 2: bob proves [finalize] importing [offer:0, accept:2]\n"
    );

    let mut bob_offer_session = build_ctx();
    let offer_returned = TransferOffer::prove(&mut bob_offer_session, context, &pick_used);
    let bob_offer_pod = solve_and_verify(bob_offer_session.builder);

    let (offer_returned, bob_offer_pod) = wire((offer_returned, bob_offer_pod));
    bob_offer_pod.pod.verify().unwrap();
    offer_returned
        .validate(&bob_offer_pod, context, pick_used.commitment())
        .unwrap();

    let mut alice_session = build_ctx();
    alice_session.builder.add_pod(bob_offer_pod).unwrap();
    let offer_pick = TransferOffer::prove(&mut alice_session, context, &pick);
    let (return_prev, return_chain) = plan.event_range(3);
    let mid_returned = erased_key_state(&pick_used);
    let (acceptance, alices_pick) = TransferAcceptance::prove(
        &mut alice_session,
        &offer_returned,
        &mid_returned,
        key_alice,
        return_prev,
        return_chain,
    );
    let st_guard_return =
        is_wood_pick_guard(&mut alice_session, &state, 3, acceptance.st_rekey.clone());
    alice_session.builder.reveal(&st_guard_return).unwrap();
    let alice_pod = solve_and_verify(alice_session.builder);

    let (offer_pick, acceptance, st_guard_return, alice_pod) =
        wire((offer_pick, acceptance, st_guard_return, alice_pod));
    alice_pod.pod.verify().unwrap();
    offer_pick
        .validate(&alice_pod, context, pick.commitment())
        .unwrap();
    acceptance
        .validate(&alice_pod, pick_returned.commitment())
        .unwrap();
    acceptance
        .validate_guard(&alice_pod, &st_guard_return)
        .unwrap();

    let mut bob = build_ctx();
    bob.builder.add_pod(alice_pod).unwrap();
    let mut tx = TxBuilder::new_from_commitments(
        &mut bob,
        &[pick.commitment()],
        state.grounding_witness(std::slice::from_ref(&pick)),
    );
    assert_eq!(plan.chain_start(), tx.chain_start);
    let mut leaf_positions = Vec::new();

    let scope = tx.begin_action();
    let (borrowed, st_rekey, h) = tx.rekey_apply(
        &mut bob,
        &offer_pick.spend_facts(),
        offer_pick.st_key_erasure.clone(),
        &erased_key_state(&pick),
        key_bob,
    );
    leaf_positions.push(tx.chain_position());
    tx.set_guard(h, is_wood_pick_guard(&mut bob, &state, 3, st_rekey));
    tx.end_action(scope);
    assert_eq!(borrowed.commitment(), pick_lent.commitment());

    let scope_outer = tx.begin_action();
    let st_use_wp = {
        let scope_sub = tx.begin_action();
        let (st_mutate, h_sub) = tx.mutate_dicts(&mut bob, &pick_used, &pick_lent);
        leaf_positions.push(tx.chain_position());
        let op_gt = bob
            .builder
            .priv_op(op!(Gt((&pick_lent, "durability"), 0_i64)))
            .unwrap();
        let op_sum = bob
            .builder
            .priv_op(op!(Sum(99_i64, 1_i64, (&pick_lent, "durability"))))
            .unwrap();
        let op_du = bob
            .builder
            .priv_op(op!(DictUpdate(pick_lent, "durability", 99_i64, pick_used)))
            .unwrap();
        let st_action = bob
            .apply_custom_pred_simple(false, "UseWoodPick", vec![op_gt, op_sum, op_du, st_mutate])
            .unwrap();
        tx.set_guard(
            h_sub,
            is_wood_pick_guard(&mut bob, &state, 2, st_action.clone()),
        );
        tx.end_action(scope_sub);
        st_action
    };
    let (_stone, st_stone_insert, h) = tx.insert(&mut bob, &stone_initial);
    leaf_positions.push(tx.chain_position());
    let st_mine = bob
        .apply_custom_pred_simple(false, "MineStone", vec![st_use_wp, st_stone_insert])
        .unwrap();
    let st_guard = bob
        .apply_custom_pred(
            false,
            "IsStone",
            map!({"state_header" => state.state_header().array()}),
            vec![st_mine],
        )
        .unwrap();
    tx.set_guard(h, st_guard);
    tx.end_action(scope_outer);

    let scope = tx.begin_action();
    let h = tx.rekey_record(&mut bob, &pick_used, &acceptance.state_facts());
    leaf_positions.push(tx.chain_position());
    tx.set_guard(h, st_guard_return);
    tx.end_action(scope);

    eprintln!("{tx}");
    let (st, tx_out, stats) = tx.finalize(&mut bob);
    print_stats(&stats);
    bob.builder.reveal(&st).unwrap();
    solve_and_verify(bob.builder);

    assert_eq!(alices_pick.commitment(), pick_returned.commitment());
    assert!(
        tx_out
            .live
            .contains(&Value::from(stone.commitment()))
            .unwrap()
    );
    assert_eq!(stats.get("EndorseSpend"), Some(&2));
    assert_eq!(jt.final_custody()[&pick_returned.commitment()], "alice");
    assert_eq!(jt.final_custody()[&stone.commitment()], "bob");
    assert_plan_agrees(plan, &leaf_positions, &tx_out);
}

/// Reject tampered contributions at receipt with specific errors.
#[test]
fn contribution_validation_rejects_mismatched_bundles() {
    let (modules, is_wood_pick) = craft_modules();
    let mut state = TestState::empty(0);
    let pick = spawn_wood_pick(&mut state, &modules, is_wood_pick);
    let receiver_key = Value::from(rand_raw_value());
    let plan = transfer_plan(&pick, &receiver_key);
    let context = plan.context(state.state_header().hash());

    let mut owner = BuildContext {
        builder: MultiPodBuilder::new(&Params::default(), &VDSet::new(&[])),
        modules,
    };
    let offer = TransferOffer::prove(&mut owner, context, &pick);
    let pod = solve_and_verify(owner.builder);

    offer.validate(&pod, context, pick.commitment()).unwrap();

    let err = offer.validate(&pod, context, test_hash(9)).unwrap_err();
    assert!(format!("{err}").contains("openings are for"));

    let err = offer
        .validate(&pod, test_hash(8), pick.commitment())
        .unwrap_err();
    assert!(format!("{err}").contains("spend endorsement is"));

    let mut wrong_nullifier = offer.clone();
    wrong_nullifier.auth.nullifier = test_hash(7);
    let err = wrong_nullifier
        .validate(&pod, context, pick.commitment())
        .unwrap_err();
    assert!(format!("{err}").contains("spend endorsement is"));

    let mut wrong_type = offer.clone();
    wrong_type.openings.type_value = Value::from(123_i64);
    let err = wrong_type
        .validate(&pod, context, pick.commitment())
        .unwrap_err();
    assert!(format!("{err}").contains("type opening is"));

    let mut unproven = offer;
    unproven.auth.st_endorsement = Statement::None;
    let err = unproven
        .validate(&pod, context, pick.commitment())
        .unwrap_err();
    assert!(format!("{err}").contains("not among the pod's public statements"));
}

/// Reject an otherwise valid endorsement bound to a different context.
/// Replay binds the real context, leaving the stale statement unusable.
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
// Plans report malformed counterparty data with errors rather than panics.
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

// Cover a direct leaf on each side of a nested action.
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
    // Actions define scopes but add no chain step.
    assert_eq!(plan.event_range(0).0, plan.chain_start());
    assert_eq!(plan.event_range(1).0, plan.event_range(0).1);
    assert_eq!(plan.event_range(2).0, plan.event_range(1).1);
    assert_eq!(plan.chain_end(), plan.event_range(2).1);
    // Both direct leaves use the outer scope.
    assert_eq!(plan.scope(0), (plan.chain_start(), plan.chain_end()));
    assert_eq!(plan.scope(0), plan.scope(2));
    // The nested leaf uses only the inner scope.
    assert_eq!(
        plan.scope(1),
        (plan.event_range(0).1, plan.event_range(1).1)
    );
}

// Pin the delete arm, which the proving scenarios do not exercise.
#[test]
fn tx_plan_delete_arm_matches_the_chain_helpers() {
    let old = test_hash(1);
    let nullifier = test_hash(2);
    let plan = TxPlan::new(
        vec![old],
        vec![PlannedEvent::Action(vec![PlannedEvent::Delete {
            old,
            nullifier,
        }])],
    )
    .unwrap();

    assert_eq!(plan.chain_start(), chain_seed(&set!(Value::from(old))));
    assert_eq!(
        plan.chain_end(),
        chain_step(plan.chain_start(), event_hash_delete(Value::from(old)))
    );
    assert_eq!(plan.live().commitment(), set!().commitment());
    assert!(plan.nullifiers().contains(&Value::from(nullifier)).unwrap());
    assert_eq!(
        plan.tx_final(),
        top_level_tx(plan.live(), plan.nullifiers()).commitment()
    );
}

/// Compare graph structure independently of presentation-only node ids.
fn graph_shape(graph: &StatementGraph) -> Vec<(String, NodeKind, Vec<NodeKind>)> {
    let nodes = graph.nodes();
    let kind_of = |id: &String| {
        nodes
            .iter()
            .find(|node| &node.id == id)
            .expect("premises name declared nodes")
            .kind
            .clone()
    };
    nodes
        .iter()
        .map(|node| {
            (
                node.producer.clone(),
                node.kind.clone(),
                node.premises.iter().map(kind_of).collect(),
            )
        })
        .collect()
}

// Pin graph derivation for transfers and finalizer-internal actions.
#[test]
fn joint_transaction_derives_the_scenario_graphs() {
    let (a1, a2) = (test_hash(1), test_hash(2));
    let (b1, b2) = (test_hash(3), test_hash(4));
    let (n1, n2) = (test_hash(5), test_hash(6));

    let swap = joint(
        &["alice", "bob"],
        "bob",
        &[(a1, "alice"), (b1, "bob")],
        vec![
            JointEvent::transfer_leg("alice", "bob", a1, a2, n1),
            JointEvent::transfer_leg("bob", "alice", b1, b2, n2),
        ],
    )
    .unwrap();
    assert_eq!(
        graph_shape(swap.graph()),
        vec![
            (
                "alice".into(),
                NodeKind::TransferOffer { object: a1 },
                vec![],
            ),
            ("bob".into(), NodeKind::TransferOffer { object: b1 }, vec![]),
            (
                "alice".into(),
                NodeKind::TransferAcceptance { object: b1 },
                vec![NodeKind::TransferOffer { object: b1 }],
            ),
            (
                "bob".into(),
                NodeKind::Finalize,
                vec![
                    NodeKind::TransferOffer { object: a1 },
                    NodeKind::TransferAcceptance { object: b1 },
                ],
            ),
        ]
    );

    let (used, stone, returned, n3) = (test_hash(7), test_hash(8), test_hash(9), test_hash(10));
    let borrow = joint(
        &["alice", "bob"],
        "bob",
        &[(a1, "alice")],
        vec![
            JointEvent::transfer_leg("alice", "bob", a1, a2, n1),
            JointEvent::Action(vec![
                JointEvent::Action(vec![JointEvent::Mutate {
                    performer: "bob".into(),
                    old: a2,
                    new: used,
                    nullifier: n2,
                    key: KeyAnnotation::Inherited,
                }]),
                JointEvent::Insert {
                    minter: "bob".into(),
                    new: stone,
                },
            ]),
            JointEvent::transfer_leg("bob", "alice", used, returned, n3),
        ],
    )
    .unwrap();
    assert_eq!(
        graph_shape(borrow.graph()),
        vec![
            (
                "alice".into(),
                NodeKind::TransferOffer { object: a1 },
                vec![],
            ),
            (
                "bob".into(),
                NodeKind::TransferOffer { object: used },
                vec![],
            ),
            (
                "alice".into(),
                NodeKind::TransferAcceptance { object: used },
                vec![NodeKind::TransferOffer { object: used }],
            ),
            (
                "bob".into(),
                NodeKind::Finalize,
                vec![
                    NodeKind::TransferOffer { object: a1 },
                    NodeKind::TransferAcceptance { object: used },
                ],
            ),
        ]
    );

    // A finalizer self-transfer requires no exchanged contribution.
    let solo = joint(
        &["bob"],
        "bob",
        &[(b1, "bob")],
        vec![JointEvent::transfer_leg("bob", "bob", b1, b2, n2)],
    )
    .unwrap();
    assert_eq!(
        graph_shape(solo.graph()),
        vec![("bob".into(), NodeKind::Finalize, vec![])]
    );
}

// Reject infeasible assignments, including through deserialization.
#[test]
fn joint_transaction_rejects_infeasible_assignments() {
    let (a1, a2, n1) = (test_hash(1), test_hash(2), test_hash(3));

    let err = joint(
        &["alice", "alice"],
        "alice",
        &[(a1, "alice")],
        vec![JointEvent::transfer_leg("alice", "alice", a1, a2, n1)],
    )
    .unwrap_err();
    assert!(format!("{err}").contains("duplicate participant"));

    let err = joint(
        &["alice"],
        "bob",
        &[(a1, "alice")],
        vec![JointEvent::transfer_leg("alice", "alice", a1, a2, n1)],
    )
    .unwrap_err();
    assert!(format!("{err}").contains("finalizer bob is not a participant"));

    let err = joint(
        &["alice"],
        "alice",
        &[(a1, "bob")],
        vec![JointEvent::transfer_leg("alice", "alice", a1, a2, n1)],
    )
    .unwrap_err();
    assert!(format!("{err}").contains("input holder bob is not a participant"));

    // Surface plan-structure errors unchanged.
    let err = joint(&["alice"], "alice", &[(a1, "alice")], vec![]).unwrap_err();
    assert!(format!("{err}").contains("at least one action"));

    let err = joint(
        &["alice", "bob"],
        "bob",
        &[(a1, "alice")],
        vec![JointEvent::transfer_leg("bob", "alice", a1, a2, n1)],
    )
    .unwrap_err();
    assert!(format!("{err}").contains("does not hold"));

    let err = joint(
        &["alice", "bob"],
        "bob",
        &[(a1, "alice")],
        vec![JointEvent::transfer_leg("alice", "carol", a1, a2, n1)],
    )
    .unwrap_err();
    assert!(format!("{err}").contains("receiver carol is not a participant"));

    let err = joint(
        &["alice", "bob"],
        "bob",
        &[],
        vec![JointEvent::Action(vec![JointEvent::Insert {
            minter: "alice".into(),
            new: a2,
        }])],
    )
    .unwrap_err();
    assert!(format!("{err}").contains("must be performed by the finalizer"));

    let err = joint(
        &["alice", "bob"],
        "bob",
        &[(a1, "alice")],
        vec![JointEvent::Action(vec![JointEvent::Mutate {
            performer: "alice".into(),
            old: a1,
            new: a2,
            nullifier: n1,
            key: KeyAnnotation::Inherited,
        }])],
    )
    .unwrap_err();
    assert!(format!("{err}").contains("must be performed by the finalizer"));

    // A non-finalizer state must cross through a transfer.
    let err = joint(
        &["alice", "bob"],
        "bob",
        &[(a1, "alice")],
        vec![JointEvent::Action(vec![JointEvent::Mutate {
            performer: "bob".into(),
            old: a1,
            new: a2,
            nullifier: n1,
            key: KeyAnnotation::Inherited,
        }])],
    )
    .unwrap_err();
    assert!(format!("{err}").contains("does not hold"));

    let err = joint(
        &["alice", "bob"],
        "bob",
        &[(a1, "alice")],
        vec![JointEvent::Action(vec![
            JointEvent::Transfer {
                sender: "alice".into(),
                receiver: "bob".into(),
                old: a1,
                new: a2,
                nullifier: n1,
            },
            JointEvent::Insert {
                minter: "bob".into(),
                new: test_hash(4),
            },
        ])],
    )
    .unwrap_err();
    assert!(format!("{err}").contains("sole event"));
}

// An unclassified key write propagates through inheritance until a fresh-key
// event restores sole custody.
#[test]
fn joint_transaction_enforces_sole_custody_of_survivors() {
    let (h1, h2, h3) = (test_hash(1), test_hash(2), test_hash(3));
    let (n1, n2) = (test_hash(4), test_hash(5));

    let unknown_mutate = JointEvent::Action(vec![JointEvent::Mutate {
        performer: "alice".into(),
        old: h1,
        new: h2,
        nullifier: n1,
        key: KeyAnnotation::Unknown,
    }]);

    let err = joint(
        &["alice"],
        "alice",
        &[(h1, "alice")],
        vec![unknown_mutate.clone()],
    )
    .unwrap_err();
    assert!(format!("{err}").contains("unclassified key write"));

    let inherited_after = JointEvent::Action(vec![JointEvent::Mutate {
        performer: "alice".into(),
        old: h2,
        new: h3,
        nullifier: n2,
        key: KeyAnnotation::Inherited,
    }]);
    let err = joint(
        &["alice"],
        "alice",
        &[(h1, "alice")],
        vec![unknown_mutate.clone(), inherited_after],
    )
    .unwrap_err();
    assert!(format!("{err}").contains("unclassified key write"));

    let fresh_after = JointEvent::Action(vec![JointEvent::Mutate {
        performer: "alice".into(),
        old: h2,
        new: h3,
        nullifier: n2,
        key: KeyAnnotation::Fresh,
    }]);
    let jt = joint(
        &["alice"],
        "alice",
        &[(h1, "alice")],
        vec![unknown_mutate.clone(), fresh_after],
    )
    .unwrap();
    assert_eq!(jt.final_custody()[&h3], "alice");

    let repair = JointEvent::transfer_leg("alice", "alice", h2, h3, n2);
    let jt = joint(
        &["alice"],
        "alice",
        &[(h1, "alice")],
        vec![unknown_mutate, repair],
    )
    .unwrap();
    assert_eq!(jt.final_custody()[&h3], "alice");
}

#[test]
fn joint_transaction_serde_round_trip() {
    let (a1, a2, n1) = (test_hash(1), test_hash(2), test_hash(3));
    let jt = joint(
        &["alice", "bob"],
        "bob",
        &[(a1, "alice")],
        vec![JointEvent::transfer_leg("alice", "bob", a1, a2, n1)],
    )
    .unwrap();

    let encoded = serde_json::to_string(&jt).unwrap();
    let decoded: JointTransaction = serde_json::from_str(&encoded).unwrap();
    // Derived views are functions of the serialized authored fields.
    assert_eq!(decoded, jt);
    assert_eq!(decoded.plan().tx_final(), jt.plan().tx_final());

    // Validate feasibility during deserialization.
    let mut doc: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    doc["finalizer"] = "carol".into();
    let err =
        serde_json::from_value::<JointTransaction>(doc).expect_err("non-participant finalizer");
    assert!(format!("{err}").contains("not a participant"));
}
