use super::*;

use common::test_state::TestState;
use txlib::StateRoot;

fn tx_hash(tx: &Tx) -> Hash {
    tx.dict().commitment()
}

fn tx_nullifiers(tx: &Tx) -> Vec<Hash> {
    tx.nullifiers
        .iter()
        .map(|nullifier| {
            let nullifier = nullifier.expect("tx nullifier should decode");
            Hash(nullifier.raw().0)
        })
        .collect()
}

fn apply_tx(state: &mut TestState, tx: &Tx) {
    state.apply_tx(tx_hash(tx), tx_nullifiers(tx));
}

fn grounding_witness(state: &TestState, inputs: &[Tx]) -> Arc<GroundingWitness> {
    state.build_grounding_witness(
        inputs,
        tx_hash,
        |block_number, transactions_root, nullifiers_root, gsrs_root, source_tx_proofs| {
            Arc::new(GroundingWitness::new(
                StateRoot::new(block_number, transactions_root, nullifiers_root, gsrs_root),
                source_tx_proofs,
            ))
        },
    )
}

#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_sdk_1() {
    let find_log_src = r#"
        fn FindLog(action) {
            var log = action.output("Log");
            log.set([["blueprint", "Log"]]);
            var work = action.intro_vdf(3, log);
            log.update("work", work);
        }

        fn CraftWood(action) {
            var log = action.input("Log");
            var wood = action.output("Wood");
            wood.set([["blueprint", "Wood"]]);
            var key = action.pow_obj_grind(wood, 9007199254740992);
            wood.update("key", key);
            action.intro_lt_eq_u256(wood, 9007199254740992);
        }

        fn CraftSticks(action) {
            var wood = action.input("Wood");
            var stick_a = action.output("Stick");
            var stick_b = action.output("Stick");
            stick_a.set([["blueprint", "Stick"]]);
            stick_b.set([["blueprint", "Stick"]]);
        }

        fn CraftWoodPick(action) {
            var wood = action.input("Wood");
            var stick = action.input("Stick");
            var pick = action.output("WoodPick");
            pick.set([
                ["blueprint", "WoodPick"],
                ["durability", 100]
            ]);
        }

        fn use_pick(action, pick, vdf_iters) {
            action.st_gt(pick.durability, 0);
            var durability = pick.get("durability");
            // durability -= 1; // Requires AST rewrite
            var_assign(durability, durability - 1);
            action.st_sum_of(pick.durability, durability, 1);
            pick.update("durability", durability);
            var key = action.random();
            pick.update("key", key);
            var work = action.intro_vdf(vdf_iters, pick);
            pick.update("work", work);
        }

        fn UseWoodPick(action) {
            var wood_pick = action.mutate("WoodPick");
            use_pick(action, wood_pick, 10);
        }
"#;

    let sdk = Sdk::default();

    let actions = &[
        "FindLog",
        "CraftWood",
        "CraftSticks",
        "CraftWoodPick",
        "UseWoodPick",
    ];
    let module = sdk.load_module_from_src_actions(find_log_src, actions);

    println!("{}", module.podlang_src);

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let [log_a] = executor.action("FindLog", vec![]).objs();
    apply_tx(&mut state, &log_a.tx);

    let executor = module.executor(true, grounding_witness(&state, &[log_a.tx.clone()]));
    let [wood_a] = executor.action("CraftWood", vec![log_a]).objs();
    apply_tx(&mut state, &wood_a.tx);

    let executor = module.executor(true, grounding_witness(&state, &[wood_a.tx.clone()]));
    let [stick_a, _stick_b] = executor.action("CraftSticks", vec![wood_a]).objs();
    apply_tx(&mut state, &stick_a.tx);

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let [log_b] = executor.action("FindLog", vec![]).objs();
    apply_tx(&mut state, &log_b.tx);

    let executor = module.executor(true, grounding_witness(&state, &[log_b.tx.clone()]));
    let [wood_b] = executor.action("CraftWood", vec![log_b]).objs();
    apply_tx(&mut state, &wood_b.tx);

    let executor = module.executor(
        true,
        grounding_witness(&state, &[wood_b.tx.clone(), stick_a.tx.clone()]),
    );
    let [wood_pick] = executor
        .action("CraftWoodPick", vec![wood_b, stick_a])
        .objs();
    apply_tx(&mut state, &wood_pick.tx);

    let executor = module.executor(true, grounding_witness(&state, &[wood_pick.tx.clone()]));
    let [wood_pick] = executor.action("UseWoodPick", vec![wood_pick]).objs();
    apply_tx(&mut state, &wood_pick.tx);
}
