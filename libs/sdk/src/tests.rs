use super::*;

use payload::test_state::TestState;
use txlib::StateHeader;

fn apply_tx(state: &mut TestState, tx: &Tx) {
    state.apply_tx(
        tx.live_commitments().unwrap(),
        tx.nullifier_hashes().unwrap(),
    );
}

fn assert_renders(module: &SdkModule, expected: &[&str]) {
    for fragment in expected {
        assert!(
            module.podlang_src.contains(fragment),
            "missing {fragment}\nactual:\n{}",
            module.podlang_src
        );
    }
}

fn grounding_witness(state: &TestState, input_commitments: &[Hash]) -> Arc<GroundingWitness> {
    state.build_grounding_witness(
        input_commitments,
        |block_meta, created_root, nullifiers_root, prior_state_history_root, created_proofs| {
            Arc::new(GroundingWitness::new(
                StateHeader::new(
                    block_meta.number as i64,
                    block_meta.timestamp as i64,
                    block_meta.hash,
                    created_root,
                    nullifiers_root,
                    prior_state_history_root,
                ),
                created_proofs,
            ))
        },
    )
}

#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_sdk_1() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn FindLog(action) {
            var log = action.output("Log");
            var work = action.intro_vdf(3, log);
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
            action.st_sum(durability, 1, pick.durability);
            pick.update("durability", durability);
            var key = action.random();
            pick.update("key", key);
            var work = action.intro_vdf(vdf_iters, pick);
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
    // The pick is mutated through the UseWoodPick sub-action.
    assert_eq!(
        action.total_mutations(),
        vec![MutatedObjectSlots {
            input_index: 0,
            output_index: 0,
        }]
    );
    assert_eq!(actions[3].total_mutations(), Vec::new());

    println!("{}", module.podlang_src);

    let mut state = TestState::default();

    println!("exe FindLog");
    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("FindLog", vec![]).unwrap();
    let log_a_tx = res.tx.clone();
    let [log_a] = res.objs();
    assert_eq!(log_a.obj.iter().count(), 3); // type, key, stable_identifier
    apply_tx(&mut state, &log_a_tx);

    println!("exe CraftWood");
    let executor = module.executor(true, grounding_witness(&state, &[log_a.obj.commitment()]));
    let res = executor.action("CraftWood", vec![log_a]).unwrap();
    let wood_a_tx = res.tx.clone();
    let [wood_a] = res.objs();
    apply_tx(&mut state, &wood_a_tx);

    println!("exe CraftSticks");
    let executor = module.executor(true, grounding_witness(&state, &[wood_a.obj.commitment()]));
    let res = executor.action("CraftSticks", vec![wood_a]).unwrap();
    let sticks_tx = res.tx.clone();
    let [stick_a, _stick_b] = res.objs();
    apply_tx(&mut state, &sticks_tx);

    println!("exe FindLog");
    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("FindLog", vec![]).unwrap();
    let log_b_tx = res.tx.clone();
    let [log_b] = res.objs();
    apply_tx(&mut state, &log_b_tx);

    println!("exe CraftWood");
    let executor = module.executor(true, grounding_witness(&state, &[log_b.obj.commitment()]));
    let res = executor.action("CraftWood", vec![log_b]).unwrap();
    let wood_b_tx = res.tx.clone();
    let [wood_b] = res.objs();
    apply_tx(&mut state, &wood_b_tx);

    println!("exe CraftWoodPick");
    let executor = module.executor(
        true,
        grounding_witness(&state, &[wood_b.obj.commitment(), stick_a.obj.commitment()]),
    );
    let res = executor
        .action("CraftWoodPick", vec![wood_b, stick_a])
        .unwrap();
    let wood_pick_tx = res.tx.clone();
    let [wood_pick] = res.objs();
    apply_tx(&mut state, &wood_pick_tx);

    println!("exe UseWoodPick");
    let executor = module.executor(
        true,
        grounding_witness(&state, &[wood_pick.obj.commitment()]),
    );
    let res = executor.action("UseWoodPick", vec![wood_pick]).unwrap();
    let wood_pick_tx = res.tx.clone();
    let [wood_pick] = res.objs();
    apply_tx(&mut state, &wood_pick_tx);

    println!("exe MineStoneWithWoodPick");
    let executor = module.executor(
        true,
        grounding_witness(&state, &[wood_pick.obj.commitment()]),
    );
    let res = executor
        .action("MineStoneWithWoodPick", vec![wood_pick])
        .unwrap();
    let stone_tx = res.tx.clone();
    let [_stone] = res.objs();
    apply_tx(&mut state, &stone_tx);
}

#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_sdk_2() {
    let manifest_src = r#"
        [plugin]
        name = "test"
        version = "0.1.0"
        module_hash = "b7492be2ac0298baddeb7647f0bace3a3127fc1e45e2b3219fcf0706ca7fd732"

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

/// A dict-field read (`obj.field`) as an intro arg compiles to an
/// anchored key, so execution must lift the intro pod's literal
/// statement to the anchored form before the action predicate is
/// proved. Exercises both arg positions at once.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_intro_dict_field_arg() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn FindOre(action) {
            var ore = action.output("Ore");
            ore.set([["grade_floor", 3], ["grade", 7]]);
        }

        fn RefineOre(action) {
            var ore = action.input("Ore");
            var metal = action.output("Metal");
            action.intro_lt_eq_u256(ore.grade_floor, ore.grade);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["FindOre", "RefineOre"])
        .unwrap();

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("FindOre", vec![]).unwrap();
    let ore_tx = res.tx.clone();
    let [ore] = res.objs();
    apply_tx(&mut state, &ore_tx);

    let executor = module.executor(true, grounding_witness(&state, &[ore.obj.commitment()]));
    let res = executor.action("RefineOre", vec![ore]).unwrap();
    let refine_tx = res.tx.clone();
    let [_metal] = res.objs();
    apply_tx(&mut state, &refine_tx);
}

/// Simplest records-form output: one output, no `.update`. The
/// post-form has no sub-field anchoring and no Intro use, so the
/// out-side wildcard collapses entirely: body refs render as `out.x`
/// and `x` does not appear in the private list.
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

    let expected = r#"record JustOutputIO = (out_x)
record JustOutputInitials = (x)

// Actions

JustOutput(io JustOutputIO, state_header StateHeader, chain0, chain, private: initials JustOutputInitials) = AND(
  tx::TxInsert(chain0, chain, initials.x, io.out_x, @self_predicate(IsFoo))
)

// Bridges

IsFooFromJustOutput(state, state_header, chain0, chain, private: io JustOutputIO) = AND(
  ArrayContains(io, JustOutputIO::out_x, state)
  JustOutput(io, state_header, chain0, chain)
)

// Classes

IsFoo(state, state_header StateHeader, chain0, chain) = OR(
  IsFooFromJustOutput(state, state_header, chain0, chain)
)"#;
    assert!(
        module.podlang_src.contains(expected),
        "records-form mismatch.\nexpected fragment:\n{expected}\nactual:\n{}",
        module.podlang_src
    );
}

/// 1 input + 1 output with `.update`.
/// - input `log` has no sub-field reads -> collapses to `in.log`,
///   no `log` wildcard, no `ArrayContains` clause.
/// - output `wood` has no sub-field reads on its post-form ->
///   collapses to `out.wood`, no `wood` wildcard.
/// - intermediate `wood0` (output initial form, ts=0) and witness
///   `key` appear as private wildcards.
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

    let expected = r#"record LogToWoodIO = (in_log, out_wood)
record LogToWoodInitials = (wood)

// Actions

LogToWood(io LogToWoodIO, state_header StateHeader, chain0, chain, private: chain1, wood0, key, initials LogToWoodInitials) = AND(
  DictUpdate(wood0, "key", key, initials.wood)
  tx::TxDelete(chain0, chain1, io.in_log, @self_predicate(IsLog))
  tx::TxInsert(chain1, chain, initials.wood, io.out_wood, @self_predicate(IsWood))
)

// Bridges

IsLogFromLogToWood(state, state_header, chain0, chain, private: io LogToWoodIO) = AND(
  ArrayContains(io, LogToWoodIO::in_log, state)
  LogToWood(io, state_header, chain0, chain)
)

IsWoodFromLogToWood(state, state_header, chain0, chain, private: io LogToWoodIO) = AND(
  ArrayContains(io, LogToWoodIO::out_wood, state)
  LogToWood(io, state_header, chain0, chain)
)

// Classes

IsLog(state, state_header StateHeader, chain0, chain) = OR(
  IsLogFromLogToWood(state, state_header, chain0, chain)
)

IsWood(state, state_header StateHeader, chain0, chain) = OR(
  IsWoodFromLogToWood(state, state_header, chain0, chain)
)"#;
    assert!(
        module.podlang_src.contains(expected),
        "records-form mismatch.\nexpected fragment:\n{expected}\nactual:\n{}",
        module.podlang_src
    );
}

/// Parent action calls a sub-action.
/// - sub-action `UseFoo` (mutate) keeps its own records (`UseFooIn`/`UseFooOut`).
/// - parent `MineBar` synthesizes private `_UseFoo_in_0`/`_UseFoo_out_0`
///   wildcards typed against the sub's record schemas; emits the call with
///   those names + the parent's chain.
/// - the script-side alias `foo = action.subaction("UseFoo")` doesn't appear
///   in the parent's predicate since it's not referenced in the parent body.
#[test]
fn test_records_form_subaction() {
    let craft_src = r#"
        fn UseFoo(action) {
            var foo = action.mutate("Foo");
            action.st_gt(foo.durability, 0);
            var dur = unsafe { foo.durability - 1 };
            action.st_sum(dur, 1, foo.durability);
            foo.update("durability", dur);
        }

        fn MineBar(action) {
            var foo = action.subaction("UseFoo");
            var bar = action.output("Bar");
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["UseFoo", "MineBar"])
        .unwrap();

    // Parent action signature + sub-action call body. `bar`'s
    // out-side collapses (no sub-field reads, no Intro use) so the
    // wildcard is dropped and body refs render as `out.bar`.
    let expected_parent = r#"MineBar(io MineBarIO, state_header StateHeader, chain0, chain, private: chain1, _UseFoo_io_0 UseFooIO, initials MineBarInitials) = AND(
  UseFoo(_UseFoo_io_0, state_header, chain0, chain1)
  tx::TxInsert(chain1, chain, initials.bar, io.out_bar, @self_predicate(IsBar))
)"#;
    assert!(
        module.podlang_src.contains(expected_parent),
        "MineBar records-form mismatch.\nexpected:\n{expected_parent}\nactual:\n{}",
        module.podlang_src
    );

    // The bridge for MineBar's direct output (`bar`) should exist.
    assert!(
        module.podlang_src.contains(
            "IsBarFromMineBar(state, state_header, chain0, chain, private: io MineBarIO) = AND("
        ),
        "missing IsBarFromMineBar bridge:\n{}",
        module.podlang_src
    );
    // Sub-action's own bridge (IsFooFromUseFoo) should also exist; sub-action
    // objects don't propagate into the parent's IsX dispatch.
    assert!(
        module.podlang_src.contains("IsFooFromUseFoo("),
        "missing IsFooFromUseFoo bridge:\n{}",
        module.podlang_src
    );
}

/// Mutate with sub-field access.
/// - `in` entry needs a wildcard (`foo0`) + `ArrayContains` clause
///   because the body reads `foo0.durability`
///   (double-anchoring isn't supported).
/// - `out` entry collapses: `foo` (post-form) is only used whole-dict,
///   so no `foo` wildcard and body refs render as `out.foo`.
/// - witness `dur` appears in the private list and in both Sum and
///   DictUpdate body slots.
#[test]
fn test_records_form_mutate() {
    let craft_src = r#"
        fn UseFoo(action) {
            var foo = action.mutate("Foo");
            action.st_gt(foo.durability, 0);
            var dur = unsafe { foo.durability - 1 };
            action.st_sum(dur, 1, foo.durability);
            foo.update("durability", dur);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["UseFoo"])
        .unwrap();

    let expected = r#"record UseFooIO = (in_foo, out_foo)

// Actions

UseFoo(io UseFooIO, state_header StateHeader, chain0, chain, private: foo0, dur) = AND(
  ArrayContains(io, UseFooIO::in_foo, foo0)
  Gt(foo0.durability, 0)
  Sum(dur, 1, foo0.durability)
  DictUpdate(foo0, "durability", dur, io.out_foo)
  tx::TxMutate(chain0, chain, foo0, io.out_foo, @self_predicate(IsFoo))
)

// Bridges

IsFooFromUseFoo(state, state_header, chain0, chain, private: io UseFooIO) = AND(
  ArrayContains(io, UseFooIO::out_foo, state)
  UseFoo(io, state_header, chain0, chain)
)

// Classes

IsFoo(state, state_header StateHeader, chain0, chain) = OR(
  IsFooFromUseFoo(state, state_header, chain0, chain)
)"#;
    assert!(
        module.podlang_src.contains(expected),
        "records-form mismatch.\nexpected fragment:\n{expected}\nactual:\n{}",
        module.podlang_src
    );
}

/// Parent reads a value off the object mutated by a sub-action: the
/// referenced alias becomes a parent wildcard pinned to the sub's
/// first out entry.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_subaction_alias_read_mutate() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn LaunchProbe(action) {
            var probe = action.output("Probe");
            probe.set([["depth", 0]]);
        }

        fn Descend(action) {
            var probe = action.mutate("Probe");
            var depth = action.random();
            probe.update("depth", depth);
        }

        fn SampleRock(action) {
            var probe = action.subaction("Descend");
            var rock = action.output("Rock");
            rock.set([["found_at_depth", probe.depth]]);
        }
    "#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["LaunchProbe", "Descend", "SampleRock"])
        .unwrap();
    println!("{}", module.podlang_src);

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("LaunchProbe", vec![]).unwrap();
    let probe_tx = res.tx.clone();
    let [probe] = res.objs();
    apply_tx(&mut state, &probe_tx);

    let executor = module.executor(true, grounding_witness(&state, &[probe.obj.commitment()]));
    let res = executor.action("SampleRock", vec![probe]).unwrap();
    let [_probe2, _rock] = res.objs();
}

/// Entry reads written into another object via set(): `fuel_before`
/// reads the pre-mutation form (forces the in-side wildcard) and
/// `fuel_after` the post-mutation form (forces the out-side wildcard).
/// Both must be emitted as Contains-backed args, not literals, to
/// match the rendered anchored-key templates.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_cross_read_into_set() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn SpawnTank(action) {
            var tank = action.output("Tank");
            tank.set([["fuel", 10]]);
        }

        fn DrawFuel(action) {
            var tank = action.mutate("Tank");
            var receipt = action.output("Receipt");
            receipt.set([["fuel_before", tank.fuel]]);
            var fuel = unsafe { tank.fuel - 1 };
            action.st_sum(fuel, 1, tank.fuel);
            tank.update("fuel", fuel);
            receipt.set([["fuel_after", tank.fuel]]);
        }
    "#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["SpawnTank", "DrawFuel"])
        .unwrap();
    println!("{}", module.podlang_src);

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("SpawnTank", vec![]).unwrap();
    let tank_tx = res.tx.clone();
    let [tank] = res.objs();
    apply_tx(&mut state, &tank_tx);

    let executor = module.executor(true, grounding_witness(&state, &[tank.obj.commitment()]));
    let res = executor.action("DrawFuel", vec![tank]).unwrap();
    let [_tank2, _receipt] = res.objs();
}

/// Direct objects declared before a subaction call, with enough events
/// to pack the parent's chain record. Events are recorded sub-actions
/// first (they run during Rhai; direct events are emitted post-Rhai),
/// so chain-ts numbering must follow emission order, not declaration
/// order.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_packed_chain_objects_before_subaction() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn SpawnShip(action) {
            var ship = action.output("Ship");
            ship.set([["fuel", 10]]);
        }

        fn BurnFuel(action) {
            var ship = action.mutate("Ship");
            var fuel = action.random();
            ship.update("fuel", fuel);
        }

        fn MineTwoRocks(action) {
            var rock_a = action.output("Rock");
            var rock_b = action.output("Rock");
            var ship = action.subaction("BurnFuel");
        }
    "#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["SpawnShip", "BurnFuel", "MineTwoRocks"])
        .unwrap();
    println!("{}", module.podlang_src);

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("SpawnShip", vec![]).unwrap();
    let ship_tx = res.tx.clone();
    let [ship] = res.objs();
    apply_tx(&mut state, &ship_tx);

    let executor = module.executor(true, grounding_witness(&state, &[ship.obj.commitment()]));
    let res = executor.action("MineTwoRocks", vec![ship]).unwrap();
    let [_ship2, _rock_a, _rock_b] = res.objs();
}

/// Two mutations in one action where the second object's update()
/// takes a value read off the first object, exercising the
/// Contains-backed value arg on the DictUpdate path.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_cross_read_into_update() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn SpawnShip(action) {
            var ship = action.output("Ship");
            ship.set([["fuel", 10]]);
        }

        fn SpawnSector(action) {
            var sector = action.output("Sector");
            sector.set([["ship_fuel", 0]]);
        }

        fn EnterSector(action) {
            var ship = action.mutate("Ship");
            var sector = action.mutate("Sector");
            var fuel = unsafe { ship.fuel - 1 };
            action.st_sum(fuel, 1, ship.fuel);
            ship.update("fuel", fuel);
            sector.update("ship_fuel", ship.fuel);
        }
    "#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["SpawnShip", "SpawnSector", "EnterSector"])
        .unwrap();
    println!("{}", module.podlang_src);

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("SpawnShip", vec![]).unwrap();
    let ship_tx = res.tx.clone();
    let [ship] = res.objs();
    apply_tx(&mut state, &ship_tx);

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("SpawnSector", vec![]).unwrap();
    let sector_tx = res.tx.clone();
    let [sector] = res.objs();
    apply_tx(&mut state, &sector_tx);

    let executor = module.executor(
        true,
        grounding_witness(&state, &[ship.obj.commitment(), sector.obj.commitment()]),
    );
    let res = executor.action("EnterSector", vec![ship, sector]).unwrap();
    let [_ship2, _sector2] = res.objs();
}

/// Verifies that object-valued writes capture the object before later
/// mutations. Covers both `set` and `update`.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_whole_object_written_before_mutation() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn SpawnShip(action) {
            var ship = action.output("Ship");
            ship.set([["fuel", 10]]);
        }

        fn LogViaSet(action) {
            var ship = action.mutate("Ship");
            var log = action.output("Log");
            log.set([["ship_before", ship]]);
            var fuel = unsafe { ship.fuel - 1 };
            action.st_sum(fuel, 1, ship.fuel);
            ship.update("fuel", fuel);
        }

        fn LogViaUpdate(action) {
            var ship = action.mutate("Ship");
            var log = action.output("Log");
            log.set([["ship_before", 0]]);
            log.update("ship_before", ship);
            var fuel = unsafe { ship.fuel - 1 };
            action.st_sum(fuel, 1, ship.fuel);
            ship.update("fuel", fuel);
        }
    "#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["SpawnShip", "LogViaSet", "LogViaUpdate"])
        .unwrap();
    println!("{}", module.podlang_src);

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("SpawnShip", vec![]).unwrap();
    let spawn_tx = res.tx.clone();
    let [ship] = res.objs();
    apply_tx(&mut state, &spawn_tx);
    let ship_before = ship.obj.clone();

    for action in ["LogViaSet", "LogViaUpdate"] {
        let executor =
            module.executor(true, grounding_witness(&state, &[ship_before.commitment()]));
        let res = executor
            .action(
                action,
                vec![SpendableObject {
                    obj: ship_before.clone(),
                }],
            )
            .unwrap();
        let [_ship2, log] = res.objs();
        let logged = log.obj.get(&StrKey::from("ship_before")).unwrap().unwrap();
        assert_eq!(
            logged,
            Value::from(ship_before.clone()),
            "{action} recorded the post-mutation ship"
        );
    }
}

/// Parent reads values off an object created (not mutated) by a
/// sub-action, exercising the post-identity rebinding of the alias.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_subaction_alias_read_output() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn SpawnShip(action) {
            var ship = action.output("Ship");
            ship.set([["fuel", 10]]);
        }

        fn ChristenShip(action) {
            var ship = action.subaction("SpawnShip");
            var plaque = action.output("Plaque");
            plaque.set([["ship_fuel", ship.fuel], ["ship_id", ship.stable_identifier]]);
        }
    "#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["SpawnShip", "ChristenShip"])
        .unwrap();
    println!("{}", module.podlang_src);

    let state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("ChristenShip", vec![]).unwrap();
    let [ship, plaque] = res.objs();
    let ship_id = ship
        .obj
        .get(&StrKey::from("stable_identifier"))
        .unwrap()
        .unwrap();
    let plaque_ship_id = plaque.obj.get(&StrKey::from("ship_id")).unwrap().unwrap();
    assert_eq!(ship_id, plaque_ship_id);
}

/// A sub-action that produces no object has no out entry to pin an
/// alias to, so a parent body that reads the alias must be rejected at
/// module load.
#[test]
fn test_subaction_alias_no_output_rejected() {
    let craft_src = r#"
        fn BurnLog(action) {
            var log = action.input("Log");
        }

        fn MineRock(action) {
            var burned = action.subaction("BurnLog");
            var rock = action.output("Rock");
            rock.set([["seen", burned.kind]]);
        }
    "#;
    let sdk = Sdk::default();
    let result = sdk.load_module_from_src_actions(craft_src, &["BurnLog", "MineRock"]);
    match result {
        Ok(_) => panic!("expected load to reject referencing a no-output sub-action alias"),
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("cannot be referenced"),
                "unexpected error: {msg}"
            );
        }
    }
}

/// Class names go straight into qualified ids (`<plugin>::<class>`) and
/// `.dobj` filename prefixes. The SDK refuses to compile a script that
/// declares a class name outside the `[A-Za-z0-9_-]` allowlist so a
/// malformed name can never reach the catalog or the filesystem in the
/// first place.
#[test]
fn test_class_name_rejects_invalid_chars() {
    let cases = [
        // (script body, what makes it invalid)
        (r#"action.output("Foo/bar");"#, "'/' in class name"),
        (r#"action.output("Foo\\bar");"#, "'\\' in class name"),
        (r#"action.output("..");"#, "'..' as class name"),
        (r#"action.output("weird:class");"#, "':' in class name"),
        (r#"action.input("with space");"#, "whitespace in class name"),
        (r#"action.mutate("");"#, "empty class name"),
    ];
    let sdk = Sdk::default();
    for (body, label) in cases {
        let craft_src = format!(
            r#"
fn Bad(action) {{
    {body}
}}
"#
        );
        let result = sdk.load_module_from_src_actions(&craft_src, &["Bad"]);
        match result {
            Ok(_) => panic!("expected SDK to reject {label}, but the script compiled"),
            Err(err) => {
                let msg = err.to_string();
                assert!(
                    msg.contains("class name"),
                    "unexpected error for {label}: {msg}"
                );
            }
        }
    }
}

#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_sdk_state_header() {
    let manifest_src = r#"
        [plugin]
        name = "test"
        version = "0.1.0"
        module_hash = "a8ae566dddbe81cdf1f7d15396eadb748cdf4f0a8976936c406199b556d62c10"

        [[classes]]
        name = "Ticker"
        emoji = "🌲"
        description = "A ticker."

        [[actions]]
        name = "MakeTicker"
        emoji = "🌲"
        description = "Make a ticker."

        [[actions]]
        name = "Tick"
        emoji = "🪵"
        description = "Tick the ticker."
    "#;

    let craft_src = r#"
        fn MakeTicker(action) {
            var ticker = action.output("Ticker");
            ticker.set([
                ["tick", 0],
                ["ts", state_header.block_timestamp]
            ]);
        }

        fn Tick(action) {
            var ticker = action.mutate("Ticker");
            var min_ts = unsafe { ticker.ts + 3600 };
            action.st_sum(ticker.ts, 3600, min_ts);
            action.st_gt(state_header.block_timestamp, min_ts);
            var tick1 = unsafe { ticker.tick + 1 };
            action.st_sum(ticker.tick, 1, tick1);
            ticker.update("tick", tick1);
            ticker.update("ts", state_header.block_timestamp);
        }
"#;

    let manifest: Manifest = toml::from_str(manifest_src).unwrap();

    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_manifest(craft_src, &manifest)
        .unwrap();

    println!("{}", module.podlang_src);

    let mut state = TestState::default();

    println!("exe MakeTicker");
    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("MakeTicker", vec![]).unwrap();
    let ticker0_tx = res.tx.clone();
    let [ticker0] = res.objs();
    apply_tx(&mut state, &ticker0_tx);

    println!("exe Tick");
    state.next_block(4000);
    let executor = module.executor(true, grounding_witness(&state, &[ticker0.obj.commitment()]));
    let res = executor.action("Tick", vec![ticker0]).unwrap();
    let ticker1_tx = res.tx.clone();
    let [_ticker1] = res.objs();
    apply_tx(&mut state, &ticker1_tx);
}

/// A whole-container statement arg renders anchored (`io.in_ore`) when
/// its Object's side collapses into the io record, so execution has to
/// lift the proved statement to the record entry the same way an intro
/// pod's is lifted.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_statement_whole_dict_arg() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn FindOre(action) {
            var ore = action.output("Ore");
            ore.set([["grade", 7], ["floor", 3]]);
        }

        fn AssertOre(action) {
            var ore = action.input("Ore");
            var metal = action.output("Metal");
            action.st_dict_contains(ore, "grade", 7);
            action.st_contains(ore, "floor", 3);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["FindOre", "AssertOre"])
        .unwrap();
    assert_renders(
        &module,
        &[
            r#"DictContains(io.in_ore, "grade", 7)"#,
            r#"Contains(io.in_ore, "floor", 3)"#,
        ],
    );

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("FindOre", vec![]).unwrap();
    let ore_tx = res.tx.clone();
    let [ore] = res.objs();
    apply_tx(&mut state, &ore_tx);

    let executor = module.executor(true, grounding_witness(&state, &[ore.obj.commitment()]));
    let res = executor.action("AssertOre", vec![ore]).unwrap();
    let metal_tx = res.tx.clone();
    apply_tx(&mut state, &metal_tx);
}

/// `*` computes a witness inside an `unsafe` block and emits nothing;
/// `st_product` is what constrains it. Proving the pair end to end is
/// what shows the new operator and its statement agree.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_unsafe_product_paired_with_statement() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn FindOre(action) {
            var ore = action.output("Ore");
            ore.set([["grade", 7]]);
        }

        fn MixAlloy(action) {
            var ore = action.mutate("Ore");
            var doubled = unsafe { ore.grade * 2 };
            action.st_product(ore.grade, 2, doubled);
            ore.update("grade", doubled);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["FindOre", "MixAlloy"])
        .unwrap();
    assert_renders(&module, &["Product(ore0.grade, 2, doubled)"]);

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("FindOre", vec![]).unwrap();
    let ore_tx = res.tx.clone();
    let [ore] = res.objs();
    apply_tx(&mut state, &ore_tx);

    let executor = module.executor(true, grounding_witness(&state, &[ore.obj.commitment()]));
    let res = executor.action("MixAlloy", vec![ore]).unwrap();
    let mixed_tx = res.tx.clone();
    let [mixed] = res.objs();
    apply_tx(&mut state, &mixed_tx);
    assert_eq!(
        mixed.obj.get(&StrKey::from("grade")).unwrap().unwrap(),
        Value::from(14)
    );
}

/// The operators emit nothing on their own, and are rejected outside an
/// `unsafe` block rather than quietly constraining their result. That
/// keeps one meaning per spelling: `unsafe` covers the dynamic extent of
/// its block, so a context-dependent operator would mean different things
/// in a script function depending on its caller.
#[test]
fn test_arithmetic_is_unsafe_only() {
    let craft_src = r#"
        fn UnsafeMix(action) {
            var ore = action.input("Ore");
            var alloy = action.output("Alloy");
            alloy.set([["grade", 0]]);
            var lowered = unsafe { ore.grade - 1 };
            alloy.update("grade", lowered);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["UnsafeMix"])
        .unwrap();
    assert!(
        !module.podlang_src.contains("Sum("),
        "unsafe subtraction should emit no statement:\n{}",
        module.podlang_src
    );

    for (action, src) in [
        (
            "BareSub",
            r#"
        fn BareSub(action) {
            var ore = action.input("Ore");
            var alloy = action.output("Alloy");
            alloy.set([["grade", 0]]);
            var lowered = ore.grade - 1;
            alloy.update("grade", lowered);
        }
"#,
        ),
        (
            "BareMul",
            r#"
        fn BareMul(action) {
            var ore = action.input("Ore");
            var alloy = action.output("Alloy");
            alloy.set([["grade", 0]]);
            var doubled = ore.grade * 2;
            alloy.update("grade", doubled);
        }
"#,
        ),
    ] {
        let err = match Sdk::default().load_module_from_src_actions(src, &[action]) {
            Ok(_) => panic!("expected {action} to require an unsafe block"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("expected unsafe block"), "{action}: {err}");
    }
}

/// Every native statement the host API exposes has to survive the round
/// trip through rendered podlang, which `load_module_from_src_actions`
/// parses and compiles. Grouped a few per action to stay inside pod2's
/// per-predicate statement budget.
#[test]
fn test_statement_surface_round_trips() {
    let craft_src = r#"
        fn Compare(action) {
            var crate_in = action.input("Crate");
            action.st_equal(crate_in.size, 2);
            action.st_not_equal(crate_in.size, 3);
        }

        fn Order(action) {
            var crate_in = action.input("Crate");
            action.st_lt(1, crate_in.size);
            action.st_lt_eq(2, 2);
            action.st_gt_eq(3, 2);
        }

        fn Arith(action) {
            var crate_in = action.input("Crate");
            action.st_product(2, 3, 6);
            action.st_max(2, 3, 3);
            action.st_hash(1, 2, crate_in.digest);
        }

        fn Reads(action) {
            var crate_in = action.input("Crate");
            action.st_not_contains(crate_in, "missing");
            action.st_dict_not_contains(crate_in, "absent");
        }

        fn SetReads(action) {
            var crate_in = action.input("Crate");
            action.st_set_contains(crate_in.tags, 1);
            action.st_set_not_contains(crate_in.tags, 2);
            action.st_array_contains(crate_in.items, 0, 1);
        }

        fn ContainerTransitions(action) {
            var crate_in = action.input("Crate");
            var crate_out = action.output("Crate");
            action.st_container_insert(crate_in, "k", 1, crate_out);
            action.st_container_update(crate_in, "k", 1, crate_out);
            action.st_container_delete(crate_in, "k", crate_out);
        }

        fn DictTransitions(action) {
            var crate_in = action.input("Crate");
            var crate_out = action.output("Crate");
            action.st_dict_insert(crate_in, "k", 1, crate_out);
            action.st_dict_update(crate_in, "k", 1, crate_out);
            action.st_dict_delete(crate_in, "k", crate_out);
        }

        fn SetTransitions(action) {
            var crate_in = action.input("Crate");
            var crate_out = action.output("Crate");
            action.st_set_insert(crate_in.tags, 1, crate_out.tags);
            action.st_set_delete(crate_in.tags, 1, crate_out.tags);
            action.st_array_update(crate_in.items, 0, 1, crate_out.items);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(
            craft_src,
            &[
                "Compare",
                "Order",
                "Arith",
                "Reads",
                "SetReads",
                "ContainerTransitions",
                "DictTransitions",
                "SetTransitions",
            ],
        )
        .unwrap();
    assert_renders(
        &module,
        &[
            "Equal(crate_in.size, 2)",
            "NotEqual(crate_in.size, 3)",
            "Lt(1, crate_in.size)",
            "LtEq(2, 2)",
            "GtEq(3, 2)",
            "Product(2, 3, 6)",
            "Max(2, 3, 3)",
            "Hash(1, 2, crate_in.digest)",
            r#"NotContains(io.in_crate_in, "missing")"#,
            r#"DictNotContains(io.in_crate_in, "absent")"#,
            "SetContains(crate_in.tags, 1)",
            "SetNotContains(crate_in.tags, 2)",
            "ArrayContains(crate_in.items, 0, 1)",
            r#"ContainerDelete(io.in_crate_in, "k", initials.crate_out)"#,
            r#"DictInsert(io.in_crate_in, "k", 1, initials.crate_out)"#,
            "SetInsert(crate_in.tags, 1, crate_out0.tags)",
            "SetDelete(crate_in.tags, 1, crate_out0.tags)",
            "ArrayUpdate(crate_in.items, 0, 1, crate_out0.items)",
        ],
    );
}

/// Reading a field of an output built in the same action. That form is
/// normally the anchored `initials.<var>` TxInsert consumes, which cannot
/// also carry a field access, so the field read has to force it open as a
/// wildcard pinned to the initials record.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_read_field_of_own_output() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn CraftPick(action) {
            var pick = action.output("Pick");
            pick.set([["durability", 100]]);
            action.st_gt(pick.durability, 0);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["CraftPick"])
        .unwrap();
    assert_renders(
        &module,
        &[
            "ArrayContains(initials, CraftPickInitials::pick, pick0)",
            r#"DictContains(pick0, "durability", 100)"#,
            "Gt(pick0.durability, 0)",
            "tx::TxInsert(chain0, chain, pick0, io.out_pick, @self_predicate(IsPick))",
        ],
    );

    let mut state = TestState::default();
    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("CraftPick", vec![]).unwrap();
    let pick_tx = res.tx.clone();
    let [pick] = res.objs();
    apply_tx(&mut state, &pick_tx);
    assert_eq!(
        pick.obj.get(&StrKey::from("durability")).unwrap().unwrap(),
        Value::from(100)
    );
}

/// `set` writes into the object's dict in place without advancing its
/// ts, so it is only sound on an output nothing has pinned yet. Repeated
/// sets stay consistent (see `test_cross_read_into_set`): containment
/// survives later inserts.
#[test]
fn test_set_guards() {
    for (action, expected, src) in [
        (
            "SetOnInput",
            "only an output object",
            r#"
        fn SetOnInput(action) {
            var ore = action.input("Ore");
            ore.set([["grade", 1]]);
        }
"#,
        ),
        (
            "SetOnMutate",
            "only an output object",
            r#"
        fn SetOnMutate(action) {
            var ore = action.mutate("Ore");
            ore.set([["grade", 1]]);
        }
"#,
        ),
        (
            "SetAfterUpdate",
            "update already committed",
            r#"
        fn SetAfterUpdate(action) {
            var ore = action.output("Ore");
            var key = action.random();
            ore.update("key", key);
            ore.set([["grade", 1]]);
        }
"#,
        ),
        (
            "SetAfterStatement",
            "a statement already committed",
            r#"
        fn SetAfterStatement(action) {
            var ore = action.output("Ore");
            ore.set([["grade", 0]]);
            action.st_dict_contains(ore, "grade", 0);
            ore.set([["grade", 1]]);
        }
"#,
        ),
        (
            "SetAfterGrind",
            "pow_obj_grind already committed",
            r#"
        fn SetAfterGrind(action) {
            var ore = action.output("Ore");
            let target = action.top_limb_u256(9007199254740992);
            var key = action.pow_obj_grind(ore, target);
            ore.set([["grade", 1]]);
        }
"#,
        ),
    ] {
        let err = match Sdk::default().load_module_from_src_actions(src, &[action]) {
            Ok(_) => panic!("expected {action} to be rejected"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains(expected), "{action}: {err}");
    }
}

/// Verifies variable-key lookup in a literal table and the constraints on
/// the returned row.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_literal_table_lookup() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn MintChart(action) {
            var chart = action.output("Chart");
            chart.set([["code", 2], ["x", 0], ["y", 0]]);
        }

        fn charts() {
            [
                #{"x": 11, "y": 12},
                #{"x": 21, "y": 22},
                #{"x": 31, "y": 32}
            ]
        }

        fn RevealChart(action) {
            var chart = action.mutate("Chart");
            var row = action.array_get(charts(), chart.code);
            chart.update("x", row.x);
            chart.update("y", row.y);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["MintChart", "RevealChart"])
        .unwrap();
    assert_renders(
        &module,
        &[
            "ArrayContains([{",
            r#""x": 11"#,
            r#""y": 32"#,
            "chart0.code, row)",
            r#"DictUpdate(chart0, "x", row.x, chart1)"#,
            r#"DictUpdate(chart1, "y", row.y, io.out_chart)"#,
        ],
    );

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("MintChart", vec![]).unwrap();
    let mint_tx = res.tx.clone();
    let [chart] = res.objs();
    apply_tx(&mut state, &mint_tx);

    let executor = module.executor(true, grounding_witness(&state, &[chart.obj.commitment()]));
    let res = executor.action("RevealChart", vec![chart]).unwrap();
    let reveal_tx = res.tx.clone();
    let [revealed] = res.objs();
    apply_tx(&mut state, &reveal_tx);
    assert_eq!(
        revealed.obj.get(&StrKey::from("x")).unwrap().unwrap(),
        Value::from(31)
    );
    assert_eq!(
        revealed.obj.get(&StrKey::from("y")).unwrap().unwrap(),
        Value::from(32)
    );
}

/// Verifies construction and membership testing of literal sets.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_literal_set_membership() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn FindOre(action) {
            var ore = action.output("Ore");
            ore.set([["grade", 5]]);
        }

        fn AssertGrade(action) {
            var ore = action.input("Ore");
            var metal = action.output("Metal");
            action.st_set_contains(set_of([3, 5, 7]), ore.grade);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["FindOre", "AssertGrade"])
        .unwrap();
    assert_renders(&module, &["SetContains(#[", "ore.grade)"]);

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("FindOre", vec![]).unwrap();
    let ore_tx = res.tx.clone();
    let [ore] = res.objs();
    apply_tx(&mut state, &ore_tx);

    let executor = module.executor(true, grounding_witness(&state, &[ore.obj.commitment()]));
    let res = executor.action("AssertGrade", vec![ore]).unwrap();
    let metal_tx = res.tx.clone();
    apply_tx(&mut state, &metal_tx);
}

/// Verifies string-keyed dictionary lookup and recursive literal promotion.
#[test]
fn test_literal_dict_nested() {
    let craft_src = r#"
        fn ReadTiers(action) {
            var ore = action.input("Ore");
            var tier = action.dict_get(#{"small": #{"cost": 1}, "large": #{"cost": 9}}, "large");
            var cost = action.dict_get(tier, "cost");
            action.st_gt(ore.grade, cost);
            action.st_gt(ore.grade, tier.cost);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["ReadTiers"])
        .unwrap();
    assert_renders(
        &module,
        &[
            r#""large": {"cost": 9}"#,
            r#""large", tier)"#,
            r#"DictContains(tier, "cost", cost)"#,
            "Gt(ore.grade, tier.cost)",
        ],
    );
}

/// Rejects variables inside container literals because their values are not
/// available during Load.
#[test]
fn test_literal_container_rejects_var() {
    let craft_src = r#"
        fn BadTable(action) {
            var ore = action.input("Ore");
            action.st_array_contains([ore.grade], 0, 3);
        }
"#;
    let err = match Sdk::default().load_module_from_src_actions(craft_src, &["BadTable"]) {
        Ok(_) => panic!("expected a var inside a container literal to be rejected"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("is a var, not a literal"), "{err}");
}

/// Renders sparse arrays with the indexed syntax added in
/// <https://github.com/0xPARC/pod2/pull/541>.
#[test]
fn test_sparse_array_literal_renders_with_indexes() {
    let dense = Array::new(vec![Value::from(1), Value::from(2)]);
    assert_eq!(
        fmt_podlang::literal_podlang(&Value::from(dense)),
        "[1, 2]".to_string()
    );

    let mut sparse = Array::empty_with_db(Box::new(pod2::middleware::db::mem::MemDB::new()));
    sparse.insert(5, Value::from(1)).unwrap();
    sparse.insert(7, Value::from(2)).unwrap();
    let sparse = Value::from(sparse);
    let literal = fmt_podlang::literal_podlang(&sparse);
    assert_eq!(literal, "[5: 1, 7: 2]");

    // The rendered source must lower back to the same container, rather than
    // merely pass the Podlang parser.
    let source = format!("my_pred(A) = AND(Equal(A, {literal}))");
    let module = pod2::lang::load_module(&source, "sparse_literal", &Params::default(), &[])
        .expect("indexed array literal lowers");
    let arg = &module.batch.predicates()[0].statements()[0].args()[1];
    let pod2::middleware::StatementTmplArg::Literal(lowered) = arg else {
        panic!("expected a literal, got {arg:?}");
    };
    assert_eq!(lowered.raw(), sparse.raw());
}

/// Reports an execution error when a variable-key lookup misses.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_container_get_rejects_missing_key() {
    let craft_src = r#"
        fn MintChart(action) {
            var chart = action.output("Chart");
            chart.set([["code", 9], ["x", 0]]);
        }

        fn RevealChart(action) {
            var chart = action.mutate("Chart");
            var row = action.array_get([#{"x": 11}, #{"x": 21}], chart.code);
            chart.update("x", row.x);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["MintChart", "RevealChart"])
        .unwrap();

    let mut state = TestState::default();
    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("MintChart", vec![]).unwrap();
    let mint_tx = res.tx.clone();
    let [chart] = res.objs();
    apply_tx(&mut state, &mint_tx);

    let executor = module.executor(true, grounding_witness(&state, &[chart.obj.commitment()]));
    let err = match executor.action("RevealChart", vec![chart]) {
        Ok(_) => panic!("expected a lookup at a key outside the table to fail"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("no entry at 9"), "{err}");
}

/// Verifies that lookup results can be used directly without an explicit
/// `var` binding.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_inline_lookup_needs_no_var_binding() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn MintChart(action) {
            var chart = action.output("Chart");
            chart.set([["code", 1], ["x", 0]]);
        }

        fn RevealChart(action) {
            var chart = action.mutate("Chart");
            chart.update("x", action.array_get([10, 20, 30], chart.code));
            action.st_gt(action.dict_get(#{"floor": 3}, "floor"), 0);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["MintChart", "RevealChart"])
        .unwrap();
    assert_renders(
        &module,
        &[
            "ArrayContains([10, 20, 30], chart0.code, _get0)",
            r#"DictUpdate(chart0, "x", _get0, io.out_chart)"#,
            r#"DictContains({"floor": 3}, "floor", _get1)"#,
            "Gt(_get1, 0)",
        ],
    );

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("MintChart", vec![]).unwrap();
    let mint_tx = res.tx.clone();
    let [chart] = res.objs();
    apply_tx(&mut state, &mint_tx);

    let executor = module.executor(true, grounding_witness(&state, &[chart.obj.commitment()]));
    let res = executor.action("RevealChart", vec![chart]).unwrap();
    let reveal_tx = res.tx.clone();
    let [revealed] = res.objs();
    apply_tx(&mut state, &reveal_tx);
    assert_eq!(
        revealed.obj.get(&StrKey::from("x")).unwrap().unwrap(),
        Value::from(20)
    );
}

/// Naming a generated variable consumes its anonymous status, regardless of
/// spelling. A later binding follows the existing named-variable rules and
/// registers another wildcard instead of renaming the original registration.
#[test]
fn test_var_binding_names_generated_vars_once() {
    for name in ["foo", "_foo", "_get0"] {
        let action = ActionHandle::new("Bind".to_string(), None);
        let mut scope = Scope::new();
        scope.push("action", action.clone());
        let _ = new_engine()
            .eval_with_scope::<Dynamic>(
                &mut scope,
                &format!("var {name} = action.array_get([10, 20], 0); var rebound = {name};"),
            )
            .unwrap();
        let ctx = action.0.borrow();
        assert_eq!(ctx.vars, ["chain", name, "rebound"], "binding {name}");
        assert!(!ctx.var_state[name].anonymous, "binding {name}");
        assert!(!ctx.var_state["rebound"].anonymous);
    }
}

/// An underscore name, including an unchanged generated name, must not let a
/// script bypass the duplicate-name error by being treated as anonymous again.
#[test]
fn test_var_binding_rejects_duplicate_named_vars() {
    for name in ["foo", "_foo", "_get0"] {
        let src = format!(
            "fn Bind(action) {{
                var {name} = action.array_get([10, 20], 0);
                var {name} = {name};
            }}"
        );
        let err = match Sdk::default().load_module_from_src_actions(&src, &["Bind"]) {
            Ok(_) => panic!("expected duplicate binding {name} to fail"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains(&format!("var {name} already exists")), "{err}");
    }
}

/// Verifies that `var` can name a literal without creating a wildcard.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_var_names_a_literal() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn FindOre(action) {
            var ore = action.output("Ore");
            ore.set([["grade", 5]]);
        }

        fn AssertGrade(action) {
            var ore = action.input("Ore");
            var metal = action.output("Metal");
            var tiers = #{"small": #{"cost": 1}, "large": #{"cost": 9}};
            var grades = set_of([3, 5, 7]);
            var floor = 1;
            var tier = action.dict_get(tiers, "small");
            action.st_set_contains(grades, ore.grade);
            action.st_gt(ore.grade, floor);
            action.st_gt(ore.grade, tier.cost);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["FindOre", "AssertGrade"])
        .unwrap();
    assert_renders(
        &module,
        &[
            r#""small": {"cost": 1}"#,
            r#""small", tier)"#,
            "SetContains(#[",
            "Gt(ore.grade, 1)",
            "Gt(ore.grade, tier.cost)",
        ],
    );

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("FindOre", vec![]).unwrap();
    let ore_tx = res.tx.clone();
    let [ore] = res.objs();
    apply_tx(&mut state, &ore_tx);

    let executor = module.executor(true, grounding_witness(&state, &[ore.obj.commitment()]));
    let res = executor.action("AssertGrade", vec![ore]).unwrap();
    let metal_tx = res.tx.clone();
    apply_tx(&mut state, &metal_tx);
}

/// Verifies that `o.get(k)` accepts computed keys and is equivalent to
/// `dict_get`.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_object_get_emits_a_lookup() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn FindOre(action) {
            var ore = action.output("Ore");
            ore.set([["grade", 5]]);
        }

        fn Weigh(action) {
            var ore = action.input("Ore");
            var metal = action.output("Metal");
            var grade = ore.get("grade");
            action.st_gt(grade, 0);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["FindOre", "Weigh"])
        .unwrap();
    assert_renders(
        &module,
        &[r#"DictContains(io.in_ore, "grade", grade)"#, "Gt(grade, 0)"],
    );

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("FindOre", vec![]).unwrap();
    let ore_tx = res.tx.clone();
    let [ore] = res.objs();
    apply_tx(&mut state, &ore_tx);

    let executor = module.executor(true, grounding_witness(&state, &[ore.obj.commitment()]));
    let res = executor.action("Weigh", vec![ore]).unwrap();
    let metal_tx = res.tx.clone();
    apply_tx(&mut state, &metal_tx);
}

/// Rejects field access on non-dictionary literals without panicking.
#[test]
fn test_field_read_on_non_dict_literal_rejected() {
    let craft_src = r#"
        fn BadRead(action) {
            var ore = action.input("Ore");
            action.st_gt(set_of([1, 2]).x, 0);
        }
"#;
    let err = match Sdk::default().load_module_from_src_actions(craft_src, &["BadRead"]) {
        Ok(_) => panic!("expected a field read on a set literal to be rejected"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("'x'") && err.contains("Set"), "{err}");
}

/// Replays a lookup against the object's pre-update value.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_lookup_before_update_reads_the_pre_update_object() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn MintChart(action) {
            var chart = action.output("Chart");
            chart.set([["code", 2], ["x", 7]]);
        }

        fn RevealChart(action) {
            var chart = action.mutate("Chart");
            action.st_gt(chart.code, 0);
            var x = action.dict_get(chart, "x");
            chart.update("x", 5);
            action.st_gt_eq(x, 0);
            action.st_gt(chart.code, 1);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["MintChart", "RevealChart"])
        .unwrap();
    assert_renders(
        &module,
        &[
            "Gt(chart0.code, 0)",
            r#"DictContains(chart0, "x", x)"#,
            r#"DictUpdate(chart0, "x", 5, chart)"#,
            "GtEq(x, 0)",
            "Gt(chart.code, 1)",
        ],
    );

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("MintChart", vec![]).unwrap();
    let mint_tx = res.tx.clone();
    let [chart] = res.objs();
    apply_tx(&mut state, &mint_tx);

    let executor = module.executor(true, grounding_witness(&state, &[chart.obj.commitment()]));
    let res = executor.action("RevealChart", vec![chart]).unwrap();
    let reveal_tx = res.tx.clone();
    let [revealed] = res.objs();
    apply_tx(&mut state, &reveal_tx);
    assert_eq!(
        revealed.obj.get(&StrKey::from("x")).unwrap().unwrap(),
        Value::from(5)
    );
}

/// Verifies nested lookups where the first lookup returns a container.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_var_container_lookup_executes() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn MintChart(action) {
            var chart = action.output("Chart");
            chart.set([["code", 1], ["cost", 0]]);
        }

        fn PriceChart(action) {
            var chart = action.mutate("Chart");
            var tiers = [#{"cost": 11}, #{"cost": 22}];
            var tier = action.array_get(tiers, chart.code);
            var cost = action.dict_get(tier, "cost");
            action.st_gt(cost, 0);
            chart.update("cost", cost);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["MintChart", "PriceChart"])
        .unwrap();
    assert_renders(
        &module,
        &[
            "chart0.code, tier)",
            r#"DictContains(tier, "cost", cost)"#,
            "Gt(cost, 0)",
            r#"DictUpdate(chart0, "cost", cost, io.out_chart)"#,
        ],
    );

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("MintChart", vec![]).unwrap();
    let mint_tx = res.tx.clone();
    let [chart] = res.objs();
    apply_tx(&mut state, &mint_tx);

    let executor = module.executor(true, grounding_witness(&state, &[chart.obj.commitment()]));
    let res = executor.action("PriceChart", vec![chart]).unwrap();
    let price_tx = res.tx.clone();
    let [priced] = res.objs();
    apply_tx(&mut state, &price_tx);
    assert_eq!(
        priced.obj.get(&StrKey::from("cost")).unwrap().unwrap(),
        Value::from(22)
    );
}

/// pod2 lowers `DictContains` and `ArrayContains` to the kind-agnostic
/// `Contains` predicate. The SDK must therefore reject mismatched container
/// kinds and key types during Load.
#[test]
fn test_container_get_checks_kind_and_key() {
    for (action, expected, src) in [
        (
            "DictOnArray",
            "DictContains: container is not a Dictionary",
            r#"
fn DictOnArray(action) {
    var ore = action.input("Ore");
    action.st_gt(action.dict_get([7, 8], 1), 0);
}
"#,
        ),
        (
            "ArrayOnDict",
            "ArrayContains: container is not an Array",
            r#"
fn ArrayOnDict(action) {
    var ore = action.input("Ore");
    action.st_gt(action.array_get(#{"k": 1}, 0), 0);
}
"#,
        ),
        (
            "ArrayOnObject",
            "ArrayContains: container is not an Array",
            r#"
fn ArrayOnObject(action) {
    var ore = action.input("Ore");
    action.st_gt(action.array_get(ore, 0), 0);
}
"#,
        ),
        (
            "StringIndex",
            "type check: expected Int",
            r#"
fn StringIndex(action) {
    var ore = action.input("Ore");
    action.st_gt(action.array_get([7, 8], "k"), 0);
}
"#,
        ),
        (
            "IntKey",
            "type check: expected Str",
            r#"
fn IntKey(action) {
    var ore = action.input("Ore");
    action.st_gt(action.dict_get(#{"grade": 7}, 0), 0);
}
"#,
        ),
        (
            "IntKeyOnObject",
            "type check: expected Str",
            r#"
fn IntKeyOnObject(action) {
    var ore = action.input("Ore");
    action.st_gt(ore.get(0), 0);
}
"#,
        ),
        (
            "IndexPastEnd",
            "ArrayContains: no entry at 5",
            r#"
fn IndexPastEnd(action) {
    var ore = action.input("Ore");
    action.st_gt(action.array_get([7, 8], 5), 0);
}
"#,
        ),
        (
            "NegativeIndex",
            "ArrayContains: no entry at -1",
            r#"
fn NegativeIndex(action) {
    var ore = action.input("Ore");
    action.st_gt(action.array_get([7, 8], -1), 0);
}
"#,
        ),
    ] {
        let err = match Sdk::default().load_module_from_src_actions(src, &[action]) {
            Ok(_) => panic!("{action}: expected the lookup to be rejected"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains(expected), "{action}: {err}");
    }
}

/// Verifies that booleans in container literals become pod2 integer values.
#[test]
fn test_container_literal_takes_a_bool() {
    let craft_src = r#"
        fn ReadFlags(action) {
            var ore = action.input("Ore");
            action.st_dict_contains(#{"ok": true, "bad": false}, "ok", 1);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["ReadFlags"])
        .unwrap();
    assert_renders(&module, &[r#""ok": 1"#, r#""bad": 0"#]);
}

/// Reports invalid field access on a lookup result as a script error.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_field_read_on_non_dict_lookup_rejected() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn MintChart(action) {
            var chart = action.output("Chart");
            chart.set([["code", 0], ["x", 0]]);
        }

        fn RevealChart(action) {
            var chart = action.mutate("Chart");
            var row = action.array_get([[11, 12]], chart.code);
            chart.update("x", row.x);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["MintChart", "RevealChart"])
        .unwrap();

    let mut state = TestState::default();
    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("MintChart", vec![]).unwrap();
    let mint_tx = res.tx.clone();
    let [chart] = res.objs();
    apply_tx(&mut state, &mint_tx);

    let executor = module.executor(true, grounding_witness(&state, &[chart.obj.commitment()]));
    let err = match executor.action("RevealChart", vec![chart]) {
        Ok(_) => panic!("expected a field read on an array row to be rejected"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("not a dictionary"), "{err}");
}

/// Some container values can be interpreted as multiple kinds. The formatter
/// chooses the first valid representation in this order: set, dictionary,
/// array. Because the verifier compares raw values, this choice does not
/// change statement semantics.
#[test]
fn test_ambiguous_nested_container_renders_as_one_kind() {
    let craft_src = r#"
        fn Probe(action) {
            var ore = action.input("Ore");
            action.st_array_contains([set_of([0]), [0]], 0, 1);
            action.st_array_contains([#{"a": "a"}, set_of(["a"])], 0, 2);
            action.st_array_contains([#{}], 0, 3);
            action.st_array_contains([[7], #{"a": 1}], 1, 4);
        }
"#;
    let module = Sdk::default()
        .load_module_from_src_actions(craft_src, &["Probe"])
        .unwrap();
    assert_renders(
        &module,
        &[
            // Set{0} and Array[0].
            "ArrayContains([#[0], #[0]], 0, 1)",
            // Dict{"a": "a"} and Set{"a"}.
            r#"ArrayContains([#["a"], #["a"]], 0, 2)"#,
            // An empty container reads as all three kinds.
            "ArrayContains([#[]], 0, 3)",
            // Unambiguous containers are unaffected.
            r#"ArrayContains([[7], {"a": 1}], 1, 4)"#,
        ],
    );
}

/// Verifies lookups and set membership on containers returned by earlier
/// lookups.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_var_array_get_and_var_set_contains() {
    let _ = env_logger::builder().is_test(true).try_init();
    let craft_src = r#"
        fn FindOre(action) {
            var ore = action.output("Ore");
            ore.set([["grade", 5]]);
        }

        fn AssertGrade(action) {
            var ore = action.input("Ore");
            var metal = action.output("Metal");
            var table = #{"rows": [7, 5], "allowed": set_of([3, 5, 7])};
            var rows = action.dict_get(table, "rows");
            var allowed = action.dict_get(table, "allowed");
            action.st_array_contains(rows, 1, ore.grade);
            action.st_set_contains(allowed, ore.grade);
            var row = action.array_get(rows, 0);
            action.st_gt(row, 0);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["FindOre", "AssertGrade"])
        .unwrap();
    assert_renders(
        &module,
        &[
            r#""rows", rows)"#,
            r#""allowed", allowed)"#,
            "ArrayContains(rows, 1, ore.grade)",
            "SetContains(allowed, ore.grade)",
            "ArrayContains(rows, 0, row)",
            "Gt(row, 0)",
        ],
    );

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("FindOre", vec![]).unwrap();
    let ore_tx = res.tx.clone();
    let [ore] = res.objs();
    apply_tx(&mut state, &ore_tx);

    let executor = module.executor(true, grounding_witness(&state, &[ore.obj.commitment()]));
    let res = executor.action("AssertGrade", vec![ore]).unwrap();
    let metal_tx = res.tx.clone();
    apply_tx(&mut state, &metal_tx);
}
