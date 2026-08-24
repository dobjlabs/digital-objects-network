//! Transaction-builder integration tests and object-derivation unit tests.
//! `TestState` retains created objects so it can produce grounding proofs.

use std::sync::Arc;

use pod2::{
    frontend::{MultiPodBuilder, Operation},
    middleware::{Params, Predicate, Statement, StrKey, VDSet, Value},
};
use pod2utils::{dict, macros::BuildContext, map, op, rand_raw_value};

use super::*;
use crate::test_support::{
    TestState, craft_modules, is_wood_pick_guard, make_object, solve_and_verify, spawn_wood_pick,
    test_hash,
};

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

// Prover-side containers and verifier-side hashes must produce the same
// context commitment.
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

/// Spawn a WoodPick, then consume it to mine stone.
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

    let builder = MultiPodBuilder::new(&params, &vd_set);
    let mut ctx = BuildContext { builder, modules };

    let mut pick_new = pick.clone();
    pick_new
        .update(&StrKey::from("durability"), &Value::from(99_i64))
        .unwrap();
    let stone_initial = make_object(is_stone.clone(), &[]);

    let inputs = vec![pick.clone()];
    let witness = state.grounding_witness(&inputs);
    let mut tx2 = TxBuilder::new(&mut ctx, &inputs, witness);

    let scope_outer = tx2.begin_action();

    let st_use_wp = {
        let scope_sub = tx2.begin_action();
        let (st_mutate, h_sub) = tx2.mutate_dicts(&mut ctx, &pick_new, &pick);
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
        st_action
    };

    let (_stone, st_stone_insert, h) = tx2.insert(&mut ctx, &stone_initial);
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
}

/// Convert a log into wood and then two sticks across three transactions.
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

    let builder = MultiPodBuilder::new(&params, &vd_set);
    let mut ctx = BuildContext { builder, modules };

    let stick_a_initial = make_object(is_stick.clone(), &[]);
    let stick_b_initial = make_object(is_stick, &[]);

    let inputs = vec![wood.clone()];
    let witness = state.grounding_witness(&inputs);
    let mut tx3 = TxBuilder::new(&mut ctx, &inputs, witness);

    let scope_outer = tx3.begin_action();

    let st_del_wood = {
        let scope_sub = tx3.begin_action();
        let (st_del, h_sub) = tx3.delete(&mut ctx, &wood);
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

    let (stick_a, st_ins_a, h_a) = tx3.insert(&mut ctx, &stick_a_initial);
    let (stick_b, st_ins_b, h_b) = tx3.insert(&mut ctx, &stick_b_initial);

    // Pack both initials to keep CraftSticks within the eight-wildcard limit,
    // then bind each TxInsert `initial` argument to its anchored entry.
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

    assert!(tx3_out.live.contains(&Value::from(stick_a)).unwrap());
    assert!(tx3_out.live.contains(&Value::from(stick_b)).unwrap());
    assert!(
        tx3_out
            .nullifiers
            .contains(&Value::from(compute_nullifier(&wood)))
            .unwrap()
    );
}

/// Exercise the recursive grounding path with three inputs.
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

    // Create three independently grounded inputs.
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

    // Ground all three inputs through the recursive predicate.
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

/// Rekey a WoodPick while preserving its identity and non-key fields.
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

    assert_ne!(compute_nullifier(&moved), compute_nullifier(&pick));
}
