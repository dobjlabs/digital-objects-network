//! Multi-party transaction scenarios, proven with `MockProver`
//! against the crafting_test predicates, plus unit tests for the plan
//! derivation.
//!
//! Every cross-party artifact passes between sessions as serialized
//! bytes and is validated on receipt, so the pod boundary in these
//! tests is a real wire boundary.

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

use crate::{PlannedEvent, TransferAcceptance, TransferOffer, TxPlan};

/// Round-trip a value through its wire encoding, so a session boundary
/// passes bytes rather than the producer's Rust values.
fn wire<T: serde::Serialize + serde::de::DeserializeOwned>(value: T) -> T {
    serde_json::from_slice(&serde_json::to_vec(&value).unwrap()).unwrap()
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
    let h = tx.rekey_send(&mut assembler, &pick, &acceptance.obj_side());
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
    let (received, st_rekey, h) = tx.rekey_receive(
        &mut receiver,
        &offer.consumed_side(),
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
///
/// Every cross-party artifact crosses the session boundary as
/// serialized bytes and is validated against its pod on receipt, so
/// the pod boundary here is a real wire boundary: no Rust value made
/// by one session reaches another.
#[test]
fn swap_follows_the_two_exchange_schedule() {
    use crate::graph::{NodeKind, StatementGraph, StatementNode};

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

    // Exchange 1, as bytes: validated against its pod before use.
    let (offer_b, bob_offer_pod) = wire((offer_b, bob_offer_pod));
    bob_offer_pod.pod.verify().unwrap();
    offer_b
        .validate(&bob_offer_pod, context, pick_b.commitment())
        .unwrap();

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

    // Exchange 2, as bytes: Alice's whole session in one message.
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
    let (bobs_pick, st_rekey, h) = tx.rekey_receive(
        &mut bob,
        &offer_a.consumed_side(),
        offer_a.st_key_erasure.clone(),
        &mid_a,
        key_b,
    );
    leaf_positions.push(tx.chain_position());
    tx.set_guard(h, is_wood_pick_guard(&mut bob, &state, 3, st_rekey));
    tx.end_action(scope);

    let scope = tx.begin_action();
    let h = tx.rekey_send(&mut bob, &pick_b, &acceptance.obj_side());
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

/// Borrow-act-return in one transaction: Alice lends her pick to Bob,
/// Bob mines a stone with it, and the worn pick returns to Alice, all
/// three as top-level actions sharing one context. The loan and the
/// return finalize together, so there is no interval where Bob holds
/// the pick without the return already proven; the price is that the
/// borrowed-period work is pre-agreed, which is what lets every
/// intermediate state's fields enter the plan.
///
/// The rekey roles reverse between the legs (Alice zero-keys on the
/// way out, Bob on the way back), and Bob must finalize because
/// recording the stone insert requires holding its dict. MineStone is
/// entirely Bob-internal, so it contributes no graph node and the
/// borrow schedules exactly like the swap. Cross-party artifacts cross
/// the session boundaries as validated bytes here too.
#[test]
fn borrow_act_return_follows_the_two_exchange_schedule() {
    use crate::graph::{NodeKind, StatementGraph, StatementNode};

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

    // Data round: Alice disclosed the pick's fields and the parties
    // agreed the borrowed-period work (one MineStone: durability 100
    // to 99, one stone minted under Bob's key), so both can project
    // every intermediate state up to its owner's key.
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

    let plan = TxPlan::new(
        vec![pick.commitment()],
        vec![
            PlannedEvent::Action(vec![PlannedEvent::Mutate {
                old: pick.commitment(),
                new: pick_lent.commitment(),
                nullifier: compute_nullifier(&pick),
            }]),
            PlannedEvent::Action(vec![
                PlannedEvent::Action(vec![PlannedEvent::Mutate {
                    old: pick_lent.commitment(),
                    new: pick_used.commitment(),
                    nullifier: compute_nullifier(&pick_lent),
                }]),
                PlannedEvent::Insert {
                    new: stone.commitment(),
                },
            ]),
            PlannedEvent::Action(vec![PlannedEvent::Mutate {
                old: pick_used.commitment(),
                new: pick_returned.commitment(),
                nullifier: compute_nullifier(&pick_used),
            }]),
        ],
    )
    .unwrap();
    let context = plan.context(state.state_header().hash());

    let graph = StatementGraph::new(vec![
        StatementNode::new(
            "offer:pick",
            "alice",
            NodeKind::TransferOffer {
                object: pick.commitment(),
            },
            &[],
        ),
        StatementNode::new(
            "offer:pick_returned",
            "bob",
            NodeKind::TransferOffer {
                object: pick_used.commitment(),
            },
            &[],
        ),
        StatementNode::new(
            "accept:pick_returned",
            "alice",
            NodeKind::TransferAcceptance {
                object: pick_used.commitment(),
            },
            &["offer:pick_returned"],
        ),
        StatementNode::new(
            "finalize",
            "bob",
            NodeKind::Finalize,
            &["offer:pick", "accept:pick_returned"],
        ),
    ])
    .unwrap();
    assert_eq!(
        graph.schedule().to_string(),
        "round 0: bob proves [offer:pick_returned]\n\
         round 1: alice proves [offer:pick, accept:pick_returned] importing [offer:pick_returned]\n\
         round 2: bob proves [finalize] importing [offer:pick, accept:pick_returned]\n"
    );

    // Round 0: Bob offers the returned state. It does not exist yet;
    // its dict is fully determined by the agreed effect plus his key,
    // and the offer's statements are hash facts about that dict.
    let mut bob_offer_session = build_ctx();
    let offer_returned = TransferOffer::prove(&mut bob_offer_session, context, &pick_used);
    let bob_offer_pod = solve_and_verify(bob_offer_session.builder);

    // Exchange 1, as bytes: validated against its pod before use.
    let (offer_returned, bob_offer_pod) = wire((offer_returned, bob_offer_pod));
    bob_offer_pod.pod.verify().unwrap();
    offer_returned
        .validate(&bob_offer_pod, context, pick_used.commitment())
        .unwrap();

    // Round 1: Alice's single session: the offer of the pick she lends
    // plus her acceptance of its return at the plan's last positions.
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

    // Exchange 2, as bytes: Alice's whole session in one message.
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

    // Round 2: Bob assembles all three actions and finalizes.
    let mut bob = build_ctx();
    bob.builder.add_pod(alice_pod).unwrap();
    let mut tx = TxBuilder::new_from_commitments(
        &mut bob,
        &[pick.commitment()],
        state.grounding_witness(std::slice::from_ref(&pick)),
    );
    assert_eq!(plan.chain_start(), tx.chain_start);
    let mut leaf_positions = Vec::new();

    // Borrow leg.
    let scope = tx.begin_action();
    let (borrowed, st_rekey, h) = tx.rekey_receive(
        &mut bob,
        &offer_pick.consumed_side(),
        offer_pick.st_key_erasure.clone(),
        &erased_key_state(&pick),
        key_bob,
    );
    leaf_positions.push(tx.chain_position());
    tx.set_guard(h, is_wood_pick_guard(&mut bob, &state, 3, st_rekey));
    tx.end_action(scope);
    assert_eq!(borrowed.commitment(), pick_lent.commitment());

    // MineStone with the borrowed pick, entirely Bob's.
    let scope_outer = tx.begin_action();
    let st_use_wp = {
        let scope_sub = tx.begin_action();
        let (st_mutate, h_sub) = tx.mutate(&mut bob, &pick_used, &pick_lent);
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

    // Return leg.
    let scope = tx.begin_action();
    let h = tx.rekey_send(&mut bob, &pick_used, &acceptance.obj_side());
    leaf_positions.push(tx.chain_position());
    tx.set_guard(h, st_guard_return);
    tx.end_action(scope);

    eprintln!("{tx}");
    let (st, tx_out, stats) = tx.finalize(&mut bob);
    print_stats(&stats);
    bob.builder.reveal(&st).unwrap();
    solve_and_verify(bob.builder);

    // Alice got her pick back with the agreed wear, Bob kept the stone.
    // Alice endorsed once, in her offer; Bob's two spends were held, so
    // replay derived both endorsements at finalize.
    assert_eq!(alices_pick.commitment(), pick_returned.commitment());
    assert!(
        tx_out
            .live
            .contains(&Value::from(stone.commitment()))
            .unwrap()
    );
    assert_eq!(stats.get("EndorseSpend"), Some(&2));
    assert_plan_agrees(&plan, &leaf_positions, &tx_out);
}

/// A tampered or mismatched contribution fails at receipt with a
/// readable error, not as an opaque solver failure at assembly time.
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

// Both the builder's recorders and the plan derive from txlib's chain
// helpers, which are the single home of the formulas. The scenarios
// above pin that agreement for inserts and mutates; nothing here
// consumes a state by deletion, so the delete arm is pinned against
// the helpers directly.
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
