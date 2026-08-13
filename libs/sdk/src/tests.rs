use super::*;

use payload::test_state::TestState;
use txlib::StateHeader;

fn apply_tx(state: &mut TestState, tx: &Tx) {
    state.apply_tx(
        tx.live_commitments().unwrap(),
        tx.nullifier_hashes().unwrap(),
    );
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
            action.st_sum(durability, 1, pick.durability);
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
    let res = executor.action("FindLog", vec![]).unwrap();
    let log_a_tx = res.tx.clone();
    let [log_a] = res.objs();
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
        module_hash = "b356b892ed13e203eddd3f0ea85c67e56ac3756ac7fe935bf387767b0dbfc2fd"

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

/// Two actions in one transaction: both objects are re-keyed together,
/// so the pair lands or fails as a unit. This is the shape a swap takes.
#[test]
fn test_two_actions_one_transaction() {
    let _ = env_logger::builder().is_test(true).try_init();
    let src = r#"
        fn SpawnLog(action) {
            var log = action.output("Log");
        }
        fn SpawnWood(action) {
            var wood = action.output("Wood");
        }
        fn ClaimLog(action) {
            var log = action.mutate("Log");
            var key = action.random();
            log.update("key", key);
        }
        fn ClaimWood(action) {
            var wood = action.mutate("Wood");
            var key = action.random();
            wood.update("key", key);
        }
    "#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(src, &["SpawnLog", "SpawnWood", "ClaimLog", "ClaimWood"])
        .unwrap();

    let mut state = TestState::default();

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("SpawnLog", vec![]).unwrap();
    let spawn_log_tx = res.tx.clone();
    let [log] = res.objs();
    apply_tx(&mut state, &spawn_log_tx);

    let executor = module.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("SpawnWood", vec![]).unwrap();
    let spawn_wood_tx = res.tx.clone();
    let [wood] = res.objs();
    apply_tx(&mut state, &spawn_wood_tx);

    // One transaction, two top-level actions.
    let witness = grounding_witness(&state, &[log.obj.commitment(), wood.obj.commitment()]);
    let executor = module.executor(true, witness);
    let res = executor
        .actions(vec![
            Invocation {
                module: module.clone(),
                action: "ClaimLog".to_string(),
                inputs: vec![log.clone()],
            },
            Invocation {
                module: module.clone(),
                action: "ClaimWood".to_string(),
                inputs: vec![wood.clone()],
            },
        ])
        .unwrap();

    let [claimed_log, claimed_wood] = res.objs();
    let nullifiers = res.tx.nullifier_hashes().unwrap();
    assert_eq!(nullifiers.len(), 2, "both inputs are spent by the one tx");
    assert!(nullifiers.contains(&txlib::object_nullifier_hash(&log.obj).unwrap()));
    assert!(nullifiers.contains(&txlib::object_nullifier_hash(&wood.obj).unwrap()));

    // Re-keying changes each commitment, and the stable identifier
    // carries across so both stay the same objects.
    let live = res.tx.live_commitments().unwrap();
    assert!(live.contains(&claimed_log.obj.commitment()));
    assert!(live.contains(&claimed_wood.obj.commitment()));
    assert_ne!(claimed_log.obj.commitment(), log.obj.commitment());
    assert_ne!(claimed_wood.obj.commitment(), wood.obj.commitment());
    let stable = |obj: &pod2::middleware::containers::Dictionary| {
        obj.get(&pod2::middleware::StrKey::from("stable_identifier"))
            .unwrap()
            .unwrap()
    };
    assert_eq!(stable(&claimed_log.obj), stable(&log.obj));
    assert_eq!(stable(&claimed_wood.obj), stable(&wood.obj));
}

/// The same transaction, but the two actions come from two separately
/// compiled plugins. This is what lets a user's own pexe compose actions
/// over classes another plugin defined.
#[test]
fn test_two_plugins_one_transaction() {
    let _ = env_logger::builder().is_test(true).try_init();
    let logs_src = r#"
        fn SpawnLog(action) {
            var log = action.output("Log");
        }
        fn ClaimLog(action) {
            var log = action.mutate("Log");
            var key = action.random();
            log.update("key", key);
        }
    "#;
    // Structurally different from the log plugin, not just differently
    // named: predicate names are not hashed, so two plugins whose
    // rendered podlang has the same shape compile to the same batch and
    // therefore to the same classes. The extra literal field is what
    // makes this a second batch.
    let gems_src = r#"
        fn SpawnGem(action) {
            var gem = action.output("Gem");
            gem.set([
                ["facets", 8]
            ]);
        }
        fn ClaimGem(action) {
            var gem = action.mutate("Gem");
            var key = action.random();
            gem.update("key", key);
        }
    "#;
    let sdk = Sdk::default();
    let logs = sdk
        .load_module_from_src_actions(logs_src, &["SpawnLog", "ClaimLog"])
        .unwrap();
    let gems = sdk
        .load_module_from_src_actions(gems_src, &["SpawnGem", "ClaimGem"])
        .unwrap();
    assert_ne!(
        logs.module().batch.id(),
        gems.module().batch.id(),
        "the two plugins must compile to distinct batches"
    );

    let mut state = TestState::default();

    let executor = logs.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("SpawnLog", vec![]).unwrap();
    let tx = res.tx.clone();
    let [log] = res.objs();
    apply_tx(&mut state, &tx);

    let executor = gems.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("SpawnGem", vec![]).unwrap();
    let tx = res.tx.clone();
    let [gem] = res.objs();
    apply_tx(&mut state, &tx);

    let witness = grounding_witness(&state, &[log.obj.commitment(), gem.obj.commitment()]);
    let executor = Executor::with_modules(vec![logs.clone(), gems.clone()], true, witness).unwrap();
    let res = executor
        .actions(vec![
            Invocation {
                module: logs.clone(),
                action: "ClaimLog".to_string(),
                inputs: vec![log.clone()],
            },
            Invocation {
                module: gems.clone(),
                action: "ClaimGem".to_string(),
                inputs: vec![gem.clone()],
            },
        ])
        .unwrap();

    let nullifiers = res.tx.nullifier_hashes().unwrap();
    assert!(nullifiers.contains(&txlib::object_nullifier_hash(&log.obj).unwrap()));
    assert!(nullifiers.contains(&txlib::object_nullifier_hash(&gem.obj).unwrap()));

    let [claimed_log, claimed_gem] = res.objs();
    let live = res.tx.live_commitments().unwrap();
    assert!(live.contains(&claimed_log.obj.commitment()));
    assert!(live.contains(&claimed_gem.obj.commitment()));
}

/// Plugin identity is structural, not nominal: predicate names are
/// metadata and are not hashed, so renaming every class and action in a
/// plugin leaves its batch id -- and therefore all of its class hashes
/// -- unchanged. Two independently authored plugins that render to the
/// same shape share an economy, and a recipe that pins a `module_hash`
/// is pinning structure rather than a name.
#[test]
fn test_batch_id_ignores_names() {
    let _ = env_logger::builder().is_test(true).try_init();
    let logs_src = r#"
        fn SpawnLog(action) {
            var log = action.output("Log");
        }
    "#;
    let renamed_src = r#"
        fn ConjureIngot(action) {
            var ingot = action.output("Ingot");
        }
    "#;
    let sdk = Sdk::default();
    let logs = sdk
        .load_module_from_src_actions(logs_src, &["SpawnLog"])
        .unwrap();
    let renamed = sdk
        .load_module_from_src_actions(renamed_src, &["ConjureIngot"])
        .unwrap();

    assert_eq!(
        logs.module().batch.id(),
        renamed.module().batch.id(),
        "renaming a class and its action must not change the batch id"
    );
    assert_eq!(
        logs.class_hash("Log").unwrap(),
        renamed.class_hash("Ingot").unwrap(),
        "structurally identical classes are the same class"
    );

    // Adding a constrained field is a structural change, so it does move
    // the batch id.
    let extra_src = r#"
        fn SpawnLog(action) {
            var log = action.output("Log");
            log.set([
                ["facets", 8]
            ]);
        }
    "#;
    let extra = sdk
        .load_module_from_src_actions(extra_src, &["SpawnLog"])
        .unwrap();
    assert_ne!(logs.module().batch.id(), extra.module().batch.id());
}
