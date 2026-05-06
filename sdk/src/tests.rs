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
    let craft_src = r#"
        fn FindLog(action) {
            var log = action.output("Log");
            var work = action.intro_vdf(3, log);
            log.update("work", work);
        }

        fn CraftWood(action) {
            var log = action.input("Log");
            var wood = action.output("Wood");
            let target = action.top_limb_u256(9007199254740992);
            var key = action.pow_obj_grind(wood, target);
            wood.update("key", key);
            action.intro_lt_eq_u256(wood, target);
        }

        fn CraftSticks(action) {
            var wood = action.input("Wood");
            var stick_a = action.output("Stick");
            var stick_b = action.output("Stick");
        }

        fn CraftWoodPick(action) {
            var wood = action.input("Wood");
            var stick = action.input("Stick");
            var pick = action.output("WoodPick");
            pick.set([["durability", 100]]);
        }

        fn use_pick(action, pick, vdf_iters) {
            action.st_gt(pick.durability, 0);
            var durability = unsafe { pick.durability - 1 };
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

        fn MineStoneWithWoodPick(action) {
            var pick = action.subaction("UseWoodPick");
            var stone = action.output("Stone");
        }
"#;

    let sdk = Sdk::default();

    let actions = &[
        "FindLog",
        "CraftWood",
        "CraftSticks",
        "CraftWoodPick",
        "UseWoodPick",
        "MineStoneWithWoodPick",
    ];
    let module = sdk
        .load_module_from_src_actions(craft_src, actions)
        .unwrap();

    fn classes<'a>(refs: impl Iterator<Item = &'a ActionObjectRef>) -> Vec<&'a str> {
        refs.map(|r| r.class.as_str()).collect()
    }
    let actions = module.actions();
    // FindLog
    let action = &actions[0];
    assert_eq!(
        classes(action.local_inputs()),
        classes(action.total_inputs())
    );
    assert_eq!(classes(action.local_inputs()), Vec::<&str>::new());
    assert_eq!(
        classes(action.local_outputs()),
        classes(action.total_outputs())
    );
    assert_eq!(classes(action.local_outputs()), vec!["Log"]);
    // CraftWood
    let action = &actions[1];
    assert_eq!(
        classes(action.local_inputs()),
        classes(action.total_inputs())
    );
    assert_eq!(classes(action.local_inputs()), vec!["Log"]);
    assert_eq!(
        classes(action.local_outputs()),
        classes(action.total_outputs())
    );
    assert_eq!(classes(action.local_outputs()), vec!["Wood"]);
    // CraftSticks
    let action = &actions[2];
    assert_eq!(
        classes(action.local_inputs()),
        classes(action.total_inputs())
    );
    assert_eq!(classes(action.local_inputs()), vec!["Wood"]);
    assert_eq!(
        classes(action.local_outputs()),
        classes(action.total_outputs())
    );
    assert_eq!(classes(action.local_outputs()), vec!["Stick", "Stick"]);
    // CraftWoodPick
    let action = &actions[3];
    assert_eq!(
        classes(action.local_inputs()),
        classes(action.total_inputs())
    );
    assert_eq!(classes(action.local_inputs()), vec!["Wood", "Stick"]);
    assert_eq!(
        classes(action.local_outputs()),
        classes(action.total_outputs())
    );
    assert_eq!(classes(action.local_outputs()), vec!["WoodPick"]);
    // UseWoodPick
    let action = &actions[4];
    assert_eq!(
        classes(action.local_inputs()),
        classes(action.total_inputs())
    );
    assert_eq!(classes(action.local_inputs()), vec!["WoodPick"]);
    assert_eq!(
        classes(action.local_outputs()),
        classes(action.total_outputs())
    );
    assert_eq!(classes(action.local_outputs()), vec!["WoodPick"]);
    // MineStoneWithWoodPick
    let action = &actions[5];
    assert_eq!(classes(action.local_inputs()), Vec::<&str>::new());
    assert_eq!(classes(action.total_inputs()), vec!["WoodPick"]);
    assert_eq!(classes(action.local_outputs()), vec!["Stone"]);
    assert_eq!(classes(action.total_outputs()), vec!["WoodPick", "Stone"]);

    println!("{}", module.podlang_src);

    let mut state = TestState::default();

    println!("exe FindLog");
    let executor = module.executor(true, grounding_witness(&state, &[]));
    let [log_a] = executor.action("FindLog", vec![]).unwrap().objs();
    apply_tx(&mut state, &log_a.tx);

    println!("exe CraftWood");
    let executor = module.executor(true, grounding_witness(&state, &[log_a.tx.clone()]));
    let [wood_a] = executor.action("CraftWood", vec![log_a]).unwrap().objs();
    apply_tx(&mut state, &wood_a.tx);

    println!("exe CraftSticks");
    let executor = module.executor(true, grounding_witness(&state, &[wood_a.tx.clone()]));
    let [stick_a, _stick_b] = executor.action("CraftSticks", vec![wood_a]).unwrap().objs();
    apply_tx(&mut state, &stick_a.tx);

    println!("exe FindLog");
    let executor = module.executor(true, grounding_witness(&state, &[]));
    let [log_b] = executor.action("FindLog", vec![]).unwrap().objs();
    apply_tx(&mut state, &log_b.tx);

    println!("exe CraftWood");
    let executor = module.executor(true, grounding_witness(&state, &[log_b.tx.clone()]));
    let [wood_b] = executor.action("CraftWood", vec![log_b]).unwrap().objs();
    apply_tx(&mut state, &wood_b.tx);

    println!("exe CraftWoodPick");
    let executor = module.executor(
        true,
        grounding_witness(&state, &[wood_b.tx.clone(), stick_a.tx.clone()]),
    );
    let [wood_pick] = executor
        .action("CraftWoodPick", vec![wood_b, stick_a])
        .unwrap()
        .objs();
    apply_tx(&mut state, &wood_pick.tx);

    println!("exe UseWoodPick");
    let executor = module.executor(true, grounding_witness(&state, &[wood_pick.tx.clone()]));
    let [wood_pick] = executor
        .action("UseWoodPick", vec![wood_pick])
        .unwrap()
        .objs();
    apply_tx(&mut state, &wood_pick.tx);

    println!("exe MineStoneWithWoodPick");
    let executor = module.executor(true, grounding_witness(&state, &[wood_pick.tx.clone()]));
    let [stone] = executor
        .action("MineStoneWithWoodPick", vec![wood_pick])
        .unwrap()
        .objs();
    apply_tx(&mut state, &stone.tx);
}

#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_sdk_2() {
    let manifest_src = r#"
        [plugin]
        name = "test"
        version = "0.1.0"
        module_hash = "e0a3963f76d01a1ba7138c327c583e955ef5bc1e94e5b56edca41ab800e3f6d1"

        [[classes]]
        name = "Log"
        emoji = "🌲"
        description = "A discovered log that can be refined into wood."

        [[classes]]
        name = "Wood"
        emoji = "🪵"
        description = "Refined wood used for sticks and basic tools."

        [[actions]]
        name = "FindLog"
        emoji = "🌲"
        description = "Discover a log object by proving a short VDF."

        [[actions]]
        name = "CraftWood"
        emoji = "🪵"
        description = "Refine one log into a wood object with PoW quality checks."
    "#;

    let craft_src = r#"
        fn FindLog(action) {
            var log = action.output("Log");
            var work = action.intro_vdf(3, log);
            log.update("work", work);
        }

        fn CraftWood(action) {
            var log = action.input("Log");
            var wood = action.output("Wood");
            let target = action.top_limb_u256(9007199254740992);
            var key = action.pow_obj_grind(wood, target);
            wood.update("key", key);
            action.intro_lt_eq_u256(wood, target);
        }
"#;

    let manifest: Manifest = toml::from_str(manifest_src).unwrap();

    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_manifest(craft_src, &manifest)
        .unwrap();

    println!("{}", module.podlang_src);
}

// An action with one chain step and no `update()` calls produces zero
// private vars. The signature must omit `private:` entirely; emitting
// `..., private: ) = AND(` makes the pod2 parser reject the module.
#[test]
fn test_action_no_private_args() {
    let craft_src = r#"
        fn JustOutput(action) {
            var x = action.output("Foo");
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["JustOutput"])
        .unwrap();
    assert!(
        module
            .podlang_src
            .contains("JustOutput(out JustOutputOut, chain0, chain) = AND("),
        "expected no `private:` clause for zero-private-var action; got:\n{}",
        module.podlang_src
    );
}

/// Phase 2A snapshot: simplest records-form output. One output, no `.update`,
/// no `.set` on the output, no inputs. The output references resolve directly
/// to `out.x` (no bridge wildcard).
#[test]
fn test_records_form_just_output() {
    let craft_src = r#"
        fn JustOutput(action) {
            var x = action.output("Foo");
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["JustOutput"])
        .unwrap();

    let expected = r#"record JustOutputOut = (x)

// Actions

JustOutput(out JustOutputOut, chain0, chain) = AND(
  DictContains(out.x, "type", @self_predicate(IsFoo))
  tx::TxInsert(chain, chain0, out.x)
)

// Bridges

IsFooFromJustOutput(state, chain0, chain, private: out JustOutputOut) = AND(
  ArrayContains(out, JustOutputOut::x, state)
  JustOutput(out, chain0, chain)
)

// Classes

IsFoo(state, chain0, chain) = OR(
  IsFooFromJustOutput(state, chain0, chain)
)
"#;
    assert!(
        module.podlang_src.contains(expected),
        "records-form mismatch.\nexpected fragment:\n{expected}\nactual:\n{}",
        module.podlang_src
    );
}

/// Phase 2A snapshot: 1 input + 1 output with `.update`. Exercises:
/// - input AKE-direct (`in.log`, no bridge)
/// - output bridge wildcard (`wood` flat, `ArrayContains` boundary)
/// - witness wildcard (`key`) preserved as flat private
#[test]
fn test_records_form_input_output_update() {
    let craft_src = r#"
        fn LogToWood(action) {
            var log = action.input("Log");
            var wood = action.output("Wood");
            var key = action.random();
            wood.update("key", key);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["LogToWood"])
        .unwrap();

    let expected = r#"record LogToWoodIn = (log)
record LogToWoodOut = (wood)

// Actions

LogToWood(in LogToWoodIn, out LogToWoodOut, chain0, chain, private: chain1, wood0, wood, key) = AND(
  ArrayContains(out, LogToWoodOut::wood, wood)
  DictUpdate(wood, wood0, "key", key)
  DictContains(in.log, "type", @self_predicate(IsLog))
  tx::TxDelete(chain1, chain0, in.log)
  DictContains(wood, "type", @self_predicate(IsWood))
  tx::TxInsert(chain, chain1, wood)
)

// Bridges

IsLogFromLogToWood(state, chain0, chain, private: in LogToWoodIn, out LogToWoodOut) = AND(
  ArrayContains(in, LogToWoodIn::log, state)
  LogToWood(in, out, chain0, chain)
)

IsWoodFromLogToWood(state, chain0, chain, private: in LogToWoodIn, out LogToWoodOut) = AND(
  ArrayContains(out, LogToWoodOut::wood, state)
  LogToWood(in, out, chain0, chain)
)

// Classes

IsLog(state, chain0, chain) = OR(
  IsLogFromLogToWood(state, chain0, chain)
)

IsWood(state, chain0, chain) = OR(
  IsWoodFromLogToWood(state, chain0, chain)
)
"#;
    assert!(
        module.podlang_src.contains(expected),
        "records-form mismatch.\nexpected fragment:\n{expected}\nactual:\n{}",
        module.podlang_src
    );
}
