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

/// Resolved imports, as `load_module_from_src_actions` wants them,
/// each bound to the alias it is listed under.
fn imports_of<'a>(
    entries: impl IntoIterator<Item = (&'a str, &'a Rc<SdkModule>)>,
) -> Vec<ModuleImport> {
    entries
        .into_iter()
        .map(|(alias, module)| ModuleImport {
            alias: alias.to_string(),
            module: module.clone(),
        })
        .collect()
}

/// The `(class, defining module)` identity of each of an action's
/// refs. None is the module the action belongs to.
fn class_identities<'a>(
    refs: impl Iterator<Item = &'a ActionObjectRef>,
) -> Vec<(&'a str, Option<Hash>)> {
    refs.map(|object_ref| {
        (
            object_ref.class.as_str(),
            object_ref
                .defining
                .as_ref()
                .map(|defining| defining.batch_id()),
        )
    })
    .collect()
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
        .load_module_from_src_actions(craft_src, actions, &[])
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
        module_hash = "75119e01ece11fd33d43bb2f68239c92450d338140f6757dd286abbb628b5712"

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
        .load_module_from_src_manifest(craft_src, &manifest, &[])
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
        .load_module_from_src_actions(craft_src, &["FindOre", "RefineOre"], &[])
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
        .load_module_from_src_actions(craft_src, &["JustOutput"], &[])
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
        .load_module_from_src_actions(craft_src, &["LogToWood"], &[])
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
        .load_module_from_src_actions(craft_src, &["UseFoo", "MineBar"], &[])
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
        .load_module_from_src_actions(craft_src, &["UseFoo"], &[])
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
        .load_module_from_src_actions(craft_src, &["LaunchProbe", "Descend", "SampleRock"], &[])
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
        .load_module_from_src_actions(craft_src, &["SpawnTank", "DrawFuel"], &[])
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
        .load_module_from_src_actions(craft_src, &["SpawnShip", "BurnFuel", "MineTwoRocks"], &[])
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
        .load_module_from_src_actions(craft_src, &["SpawnShip", "SpawnSector", "EnterSector"], &[])
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
        .load_module_from_src_actions(craft_src, &["SpawnShip", "ChristenShip"], &[])
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
    let result = sdk.load_module_from_src_actions(craft_src, &["BurnLog", "MineRock"], &[]);
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
        let result = sdk.load_module_from_src_actions(&craft_src, &["Bad"], &[]);
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
        .load_module_from_src_manifest(craft_src, &manifest, &[])
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
        .load_module_from_src_actions(craft_src, &["FindOre", "AssertOre"], &[])
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
            ore.update("work", doubled);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["FindOre", "MixAlloy"], &[])
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
        mixed.obj.get(&StrKey::from("work")).unwrap().unwrap(),
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
            var lowered = unsafe { ore.grade - 1 };
            alloy.update("work", lowered);
        }
"#;
    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(craft_src, &["UnsafeMix"], &[])
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
            var lowered = ore.grade - 1;
            alloy.update("work", lowered);
        }
"#,
        ),
        (
            "BareMul",
            r#"
        fn BareMul(action) {
            var ore = action.input("Ore");
            var alloy = action.output("Alloy");
            var doubled = ore.grade * 2;
            alloy.update("work", doubled);
        }
"#,
        ),
    ] {
        let err = match Sdk::default().load_module_from_src_actions(src, &[action], &[]) {
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
            &[],
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
        .load_module_from_src_actions(craft_src, &["CraftPick"], &[])
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
            ore.update("work", key);
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
            action.st_dict_contains(ore, "work", 0);
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
        let err = match Sdk::default().load_module_from_src_actions(src, &[action], &[]) {
            Ok(_) => panic!("expected {action} to be rejected"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains(expected), "{action}: {err}");
    }
}

/// Cross-plugin action calls end to end: an importer plugin runs an
/// imported plugin's action as a sub-action, reads its output's
/// fields, and produces a native-class object in the same tx. The
/// foreign-produced object then grounds and spends normally through
/// the defining plugin. Both modules deliberately define an action
/// named `CraftWood` so name resolution across batches is pinned.
#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_cross_plugin_subaction() {
    let _ = env_logger::builder().is_test(true).try_init();
    let basics_src = r#"
        fn FindLog(action) {
            var log = action.output("Log");
            var work = action.intro_vdf(3, log);
            log.update("work", work);
        }

        fn CraftWood(action) {
            var log = action.input("Log");
            var wood = action.output("Wood");
        }

        fn CraftSticks(action) {
            var wood = action.input("Wood");
            var stick = action.output("Stick");
        }
    "#;
    let sdk = Sdk::default();
    let basics = sdk
        .load_module_from_src_actions(basics_src, &["FindLog", "CraftWood", "CraftSticks"], &[])
        .unwrap();

    let totem_src = r#"
        fn CraftWood(action) {
            var timber = action.output("Timber");
        }

        fn CraftTotem(action) {
            var wood = action.subaction("craft-basics::CraftWood");
            var totem = action.output("Totem");
            totem.set([["wood_key", wood.key]]);
        }
    "#;
    let imports = imports_of([("craft-basics", &basics)]);
    let totem_module = sdk
        .load_module_from_src_actions(totem_src, &["CraftWood", "CraftTotem"], &imports)
        .unwrap();

    println!("{}", totem_module.podlang_src());
    let src = totem_module.podlang_src();
    assert!(
        src.contains(&format!(
            "use module {:#} as craft_basics",
            basics.module().batch.id()
        )),
        "importer podlang must import the basics batch"
    );
    assert!(src.contains("craft_basics::CraftWood(_craft_basics_CraftWood_io_0"));
    assert!(src.contains("_craft_basics_CraftWood_io_0 craft_basics::CraftWoodIO"));

    // The sub-action's inputs/outputs splice into the parent's totals,
    // tagged with the defining plugin.
    let meta = totem_module
        .actions()
        .iter()
        .find(|a| a.name == "CraftTotem")
        .unwrap();
    assert_eq!(
        class_identities(meta.total_inputs()),
        vec![("Log", Some(basics.module().batch.id()))]
    );
    assert_eq!(
        class_identities(meta.total_outputs()),
        vec![("Wood", Some(basics.module().batch.id())), ("Totem", None),]
    );

    // Same class name, different plugin, different guard hash.
    assert_ne!(
        basics.class_hash("Wood").unwrap(),
        totem_module.class_hash("Timber").unwrap()
    );

    // Rendering a foreign predicate qualifies it the way the podlang
    // above spells it, not the way the plugin name is spelled.
    assert_eq!(
        totem_module
            .module_aliases()
            .get(&basics.module().batch.id())
            .map(String::as_str),
        Some("craft_basics")
    );

    let mut state = TestState::default();

    println!("exe FindLog (basics)");
    let executor = basics.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("FindLog", vec![]).unwrap();
    let log_tx = res.tx.clone();
    let [log] = res.objs();
    apply_tx(&mut state, &log_tx);

    println!("exe CraftTotem (importer, sub-calls basics::CraftWood)");
    let executor = totem_module.executor(true, grounding_witness(&state, &[log.obj.commitment()]));
    let res = executor.action("CraftTotem", vec![log]).unwrap();
    let totem_tx = res.tx.clone();
    let [wood, totem] = res.objs();
    apply_tx(&mut state, &totem_tx);

    // The foreign-produced wood carries the basics IsWood guard hash;
    // the native totem carries the importer's IsTotem hash and links
    // the wood's key.
    assert_eq!(
        txlib::object_type(&wood.obj),
        Value::from(basics.class_hash("Wood").unwrap())
    );
    assert_eq!(
        txlib::object_type(&totem.obj),
        Value::from(totem_module.class_hash("Totem").unwrap())
    );
    assert_eq!(
        totem.obj.get(&StrKey::from("wood_key")).unwrap().unwrap(),
        wood.obj.get(&StrKey::from("key")).unwrap().unwrap()
    );

    println!("exe CraftSticks (basics, spends the importer-tx wood)");
    let executor = basics.executor(true, grounding_witness(&state, &[wood.obj.commitment()]));
    let res = executor.action("CraftSticks", vec![wood]).unwrap();
    let sticks_tx = res.tx.clone();
    let [_stick] = res.objs();
    apply_tx(&mut state, &sticks_tx);
}

/// A qualified sub-action must name a declared import; a foreign class
/// can never be declared directly (closed classes).
#[test]
fn test_cross_plugin_load_errors() {
    let sdk = Sdk::default();

    let undeclared_src = r#"
        fn UsesGhost(action) {
            var x = action.subaction("ghost-plugin::Conjure");
        }
    "#;
    let err = sdk
        .load_module_from_src_actions(undeclared_src, &["UsesGhost"], &[])
        .err()
        .expect("undeclared plugin must fail to load");
    assert!(
        err.to_string().contains("not declared as an import"),
        "unexpected error: {err}"
    );

    let foreign_class_src = r#"
        fn StealWood(action) {
            var wood = action.input("craft-basics::Wood");
        }
    "#;
    let err = sdk
        .load_module_from_src_actions(foreign_class_src, &["StealWood"], &[])
        .err()
        .expect("foreign class declaration must fail to load");
    assert!(
        err.to_string().contains("foreign classes are closed"),
        "unexpected error: {err}"
    );
}

/// Manifest `[[imports]]` validation: the declared name/hash pins must
/// line up with the modules the loader was handed.
#[test]
fn test_cross_plugin_manifest_imports() {
    let sdk = Sdk::default();
    let basics_src = r#"
        fn FindLog(action) {
            var log = action.output("Log");
        }
    "#;
    let basics = sdk
        .load_module_from_src_actions(basics_src, &["FindLog"], &[])
        .unwrap();
    let basics_hash = format!("{:#}", basics.module().batch.id());
    let imports = imports_of([("craft-basics", &basics)]);
    let imports = imports.as_slice();

    let parent_src = r#"
        fn Beacon(action) {
            var log = action.subaction("craft-basics::FindLog");
            var beacon = action.output("Beacon");
        }
    "#;
    let manifest_toml = |import_hash: &str| {
        format!(
            r#"
        [plugin]
        name = "beacon"
        version = "0.1.0"
        module_hash = "0000000000000000000000000000000000000000000000000000000000000000"

        [[imports]]
        name = "craft-basics"
        module_hash = "{}"

        [[classes]]
        name = "Beacon"
        emoji = "B"
        description = "A beacon."

        [[actions]]
        name = "Beacon"
        emoji = "B"
        description = "Raise a beacon over a fresh log."
        "#,
            import_hash.trim_start_matches("0x")
        )
    };

    // A wrong pin fails before compilation.
    let manifest: Manifest = toml::from_str(&manifest_toml(&"ab".repeat(32))).unwrap();
    let err = sdk
        .load_module_from_src_manifest(parent_src, &manifest, imports)
        .err()
        .expect("pin mismatch must fail");
    assert!(
        err.to_string().contains("manifest pins module_hash"),
        "unexpected error: {err}"
    );

    // Providing a module the manifest does not declare fails too.
    let mut undeclared = toml::from_str::<Manifest>(&manifest_toml(&basics_hash)).unwrap();
    undeclared.imports.clear();
    let err = sdk
        .load_module_from_src_manifest(parent_src, &undeclared, imports)
        .err()
        .expect("undeclared provided import must fail");
    assert!(
        err.to_string().contains("does not declare it"),
        "unexpected error: {err}"
    );

    // Correct pin loads; the module hash check then runs as usual and
    // reports the real hash, which covers the import pin transitively.
    let manifest: Manifest = toml::from_str(&manifest_toml(&basics_hash)).unwrap();
    let err = sdk
        .load_module_from_src_manifest(parent_src, &manifest, imports)
        .err()
        .expect("placeholder plugin hash must mismatch");
    assert!(
        err.to_string().contains("module_hash"),
        "unexpected error: {err}"
    );
}

/// Transitive imports: C imports B, B imports A. Running C's action
/// sub-calls into B, whose body sub-calls into A, so the executor must
/// carry all three batches and each nested sub-action must resolve
/// against its own module's imports.
#[test]
fn test_transitive_plugin_imports() {
    let _ = env_logger::builder().is_test(true).try_init();
    let sdk = Sdk::default();

    let quarry_src = r#"
        fn QuarryStone(action) {
            var stone = action.output("Stone");
        }
    "#;
    let quarry = sdk
        .load_module_from_src_actions(quarry_src, &["QuarryStone"], &[])
        .unwrap();

    let mason_src = r#"
        fn CarveBlock(action) {
            var stone = action.subaction("quarry::QuarryStone");
            var block = action.output("Block");
            block.set([["stone_key", stone.key]]);
        }
    "#;
    let mason = sdk
        .load_module_from_src_actions(
            mason_src,
            &["CarveBlock"],
            &imports_of([("quarry", &quarry)]),
        )
        .unwrap();

    let builder_src = r#"
        fn RaiseWall(action) {
            var block = action.subaction("mason::CarveBlock");
            var wall = action.output("Wall");
            wall.set([["block_key", block.key]]);
        }
    "#;
    let builder = sdk
        .load_module_from_src_actions(
            builder_src,
            &["RaiseWall"],
            &imports_of([("mason", &mason)]),
        )
        .unwrap();

    // The transitively-produced Stone keeps its original defining plugin.
    let meta = builder
        .actions()
        .iter()
        .find(|a| a.name == "RaiseWall")
        .unwrap();
    assert_eq!(
        class_identities(meta.total_outputs()),
        vec![
            ("Stone", Some(quarry.module().batch.id())),
            ("Block", Some(mason.module().batch.id())),
            ("Wall", None),
        ]
    );

    // Both imported batches are qualifiable, the transitive one
    // included: a bare `QuarryStone` in a rendered statement would read
    // as a predicate the builder itself defines.
    let aliases = builder.module_aliases();
    assert_eq!(
        aliases.get(&mason.module().batch.id()).map(String::as_str),
        Some("mason")
    );
    assert_eq!(
        aliases.get(&quarry.module().batch.id()).map(String::as_str),
        Some("quarry")
    );
    assert!(!aliases.contains_key(&builder.module().batch.id()));

    let state = TestState::default();
    let executor = builder.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("RaiseWall", vec![]).unwrap();
    let [stone, block, wall] = res.objs();
    assert_eq!(
        txlib::object_type(&stone.obj),
        Value::from(quarry.class_hash("Stone").unwrap())
    );
    assert_eq!(
        txlib::object_type(&block.obj),
        Value::from(mason.class_hash("Block").unwrap())
    );
    assert_eq!(
        txlib::object_type(&wall.obj),
        Value::from(builder.class_hash("Wall").unwrap())
    );
}

/// A parent-local output declared *before* a sub-action call. The sub
/// runs during the parent's rhai body, so its produced dicts have to be
/// spliced in at the call site's position rather than accumulating ahead
/// of the parent's own: `driver::save_results` pairs the returned
/// objects with `total_outputs` by index, so a mismatch stamps each
/// object with the other one's class and filename.
#[test]
fn test_output_order_local_declared_before_subaction() {
    let _ = env_logger::builder().is_test(true).try_init();
    let sdk = Sdk::default();

    let quarry_src = r#"
        fn QuarryStone(action) {
            var stone = action.output("Stone");
        }
    "#;
    let quarry = sdk
        .load_module_from_src_actions(quarry_src, &["QuarryStone"], &[])
        .unwrap();

    let mason_src = r#"
        fn CarveBlock(action) {
            var block = action.output("Block");
            var stone = action.subaction("quarry::QuarryStone");
            block.set([["stone_key", stone.key]]);
        }
    "#;
    let mason = sdk
        .load_module_from_src_actions(
            mason_src,
            &["CarveBlock"],
            &imports_of([("quarry", &quarry)]),
        )
        .unwrap();

    let meta = mason
        .actions()
        .iter()
        .find(|a| a.name == "CarveBlock")
        .unwrap();
    assert_eq!(
        class_identities(meta.total_outputs()),
        vec![("Block", None), ("Stone", Some(quarry.module().batch.id()))]
    );

    let state = TestState::default();
    let executor = mason.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("CarveBlock", vec![]).unwrap();
    let [block, stone] = res.objs();
    assert_eq!(
        txlib::object_type(&block.obj),
        Value::from(mason.class_hash("Block").unwrap()),
        "produced objects must come back in total_outputs order"
    );
    assert_eq!(
        txlib::object_type(&stone.obj),
        Value::from(quarry.class_hash("Stone").unwrap())
    );
    assert_eq!(
        block.obj.get(&StrKey::from("stone_key")).unwrap().unwrap(),
        stone.obj.get(&StrKey::from("key")).unwrap().unwrap()
    );
}

/// The same alias bound to two different modules in one graph. C binds
/// `gem` to gem@v2 directly, and imports B, which binds `gem` to
/// gem@v1. Both are legitimate: an alias belongs to the binding that
/// declared it, so the two say nothing about each other. Each spliced
/// class must keep the module that actually defines it, and the alias
/// in a script must resolve against the imports of the module the
/// script belongs to.
#[test]
fn test_same_alias_two_modules_stay_distinct() {
    let _ = env_logger::builder().is_test(true).try_init();
    let sdk = Sdk::default();

    let gem_v1_src = r#"
        fn MintGem(action) {
            var gem = action.output("Gem");
        }
    "#;
    let gem_v2_src = r#"
        fn MintGem(action) {
            var gem = action.output("Gem");
            gem.set([["carat", 2]]);
        }
    "#;
    let gem_v1 = sdk
        .load_module_from_src_actions(gem_v1_src, &["MintGem"], &[])
        .unwrap();
    let gem_v2 = sdk
        .load_module_from_src_actions(gem_v2_src, &["MintGem"], &[])
        .unwrap();
    assert_ne!(
        gem_v1.module().batch.id(),
        gem_v2.module().batch.id(),
        "the two gem modules must differ for this to test anything"
    );

    let jeweler_src = r#"
        fn SetStone(action) {
            var gem = action.subaction("gem::MintGem");
            var setting = action.output("Setting");
        }
    "#;
    let jeweler = sdk
        .load_module_from_src_actions(jeweler_src, &["SetStone"], &imports_of([("gem", &gem_v1)]))
        .unwrap();

    // Which module an importer was built against is baked into its own
    // batch id: the predicates it compiles to reference the imported
    // batch, so swapping the import changes the importer's hash. This
    // is what makes a pin meaningful and a claimed name not worth
    // trusting past the moment it is resolved.
    let jeweler_on_v2 = sdk
        .load_module_from_src_actions(jeweler_src, &["SetStone"], &imports_of([("gem", &gem_v2)]))
        .unwrap();
    assert_ne!(
        jeweler.module().batch.id(),
        jeweler_on_v2.module().batch.id(),
        "an importer's hash must commit to the module it imported"
    );

    // The direct `gem` binding is declared second, after the import
    // whose own subtree binds the same alias to a different module.
    let crown_src = r#"
        fn ForgeCrown(action) {
            var setting = action.subaction("jeweler::SetStone");
            var gem = action.subaction("gem::MintGem");
            var crown = action.output("Crown");
        }
    "#;
    let crown = sdk
        .load_module_from_src_actions(
            crown_src,
            &["ForgeCrown"],
            &imports_of([("jeweler", &jeweler), ("gem", &gem_v2)]),
        )
        .expect("two modules under one alias is not a conflict");

    // Two Gem classes, each keeping its own defining module: the first
    // spliced up through jeweler (v1), the second from C's own binding.
    let meta = crown
        .actions()
        .iter()
        .find(|a| a.name == "ForgeCrown")
        .unwrap();
    assert_eq!(
        class_identities(meta.total_outputs()),
        vec![
            ("Gem", Some(gem_v1.module().batch.id())),
            ("Setting", Some(jeweler.module().batch.id())),
            ("Gem", Some(gem_v2.module().batch.id())),
            ("Crown", None),
        ]
    );

    // And each resolves to that module, not to whichever one an alias
    // lookup would have reached first.
    let gems: Vec<&ActionObjectRef> = meta
        .total_outputs()
        .filter(|object_ref| object_ref.class == "Gem")
        .collect();
    assert_eq!(
        crown.class_module(gems[0]).class_hash("Gem"),
        gem_v1.class_hash("Gem")
    );
    assert_eq!(
        crown.class_module(gems[1]).class_hash("Gem"),
        gem_v2.class_hash("Gem")
    );
    assert_ne!(gem_v1.class_hash("Gem"), gem_v2.class_hash("Gem"));

    // Executing agrees: the alias in C's script means C's binding, and
    // jeweler's means jeweler's.
    let state = TestState::default();
    let executor = crown.executor(true, grounding_witness(&state, &[]));
    let res = executor.action("ForgeCrown", vec![]).unwrap();
    let [gem_from_jeweler, setting, gem_direct, crown_obj] = res.objs();
    assert_eq!(
        txlib::object_type(&gem_from_jeweler.obj),
        Value::from(gem_v1.class_hash("Gem").unwrap())
    );
    assert_eq!(
        txlib::object_type(&setting.obj),
        Value::from(jeweler.class_hash("Setting").unwrap())
    );
    assert_eq!(
        txlib::object_type(&gem_direct.obj),
        Value::from(gem_v2.class_hash("Gem").unwrap())
    );
    assert_eq!(
        txlib::object_type(&crown_obj.obj),
        Value::from(crown.class_hash("Crown").unwrap())
    );
}

/// A spliced class prints as `<alias>@<batch prefix>::<Class>`: the
/// alias is the readable half and the batch prefix is what actually
/// tells two same-aliased modules apart.
#[test]
fn test_spliced_class_display_disambiguates() {
    let sdk = Sdk::default();
    let quarry = sdk
        .load_module_from_src_actions(
            r#"fn QuarryStone(action) { var stone = action.output("Stone"); }"#,
            &["QuarryStone"],
            &[],
        )
        .unwrap();
    let mason = sdk
        .load_module_from_src_actions(
            r#"fn CarveBlock(action) {
                 var stone = action.subaction("quarry::QuarryStone");
                 var block = action.output("Block");
               }"#,
            &["CarveBlock"],
            &imports_of([("quarry", &quarry)]),
        )
        .unwrap();

    let meta = mason.action_by_name("CarveBlock");
    let rendered: Vec<String> = meta.total_outputs().map(|r| r.to_string()).collect();
    let prefix: String = format!("{:#}", quarry.module().batch.id())
        .trim_start_matches("0x")
        .chars()
        .take(8)
        .collect();
    assert_eq!(
        rendered,
        vec![format!("quarry@{prefix}::Stone"), "Block".to_string()]
    );
}

/// Import alias validation, which every path into the loader shares.
/// A rejected alias would otherwise reach the podlang render and fail
/// pod2's parser, where there is no plugin name left to blame.
#[test]
fn test_import_alias_rejections() {
    let sdk = Sdk::default();
    let basics_src = r#"
        fn FindLog(action) {
            var log = action.output("Log");
        }
    "#;
    let basics = sdk
        .load_module_from_src_actions(basics_src, &["FindLog"], &[])
        .unwrap();
    // Distinct module so a duplicate-alias case is not also a
    // conflicting-versions case.
    let other_src = r#"
        fn FindOre(action) {
            var ore = action.output("Ore");
        }
    "#;
    let other = sdk
        .load_module_from_src_actions(other_src, &["FindOre"], &[])
        .unwrap();

    // Each script calls its imports so alias validation, not the
    // unused-import check, is what rejects the load.
    let one_import = r#"
        fn Dig(action) {
            var log = action.subaction("PLUGIN::FindLog");
        }
    "#;
    let two_imports = r#"
        fn Dig(action) {
            var log = action.subaction("craft-basics::FindLog");
            var ore = action.subaction("craft_basics::FindOre");
        }
    "#;

    // (plugin names, script, expected error fragment)
    let cases: [(&[&str], &str, &str); 6] = [
        (&["record"], one_import, "podlang reserves"),
        (&["private"], one_import, "podlang reserves"),
        (&["tx"], one_import, "reserved tx module alias"),
        (&["2fast"], one_import, "valid podlang module alias"),
        (&["craft.basics"], one_import, "valid podlang module alias"),
        (
            &["craft-basics", "craft_basics"],
            two_imports,
            "collides with another import",
        ),
    ];
    for (names, script, expected) in cases {
        let modules = [&basics, &other];
        let imports = imports_of(
            names
                .iter()
                .zip(modules)
                .map(|(name, module)| (*name, module)),
        );
        let script = script.replace("PLUGIN", names[0]);
        let err = sdk
            .load_module_from_src_actions(&script, &["Dig"], &imports)
            .err()
            .unwrap_or_else(|| panic!("import named {names:?} must fail to load"));
        assert!(
            err.to_string().contains(expected),
            "import named {names:?}: expected {expected:?}, got: {err}"
        );
    }
}

/// A declared import no action calls loads (with a warning), and the
/// asymmetry that makes it worth warning about: the `use module` line
/// it emits is a load-time requirement, while the batch id -- a merkle
/// root over the predicates -- is identical with or without it. The
/// pin can be repointed without changing the module hash.
#[test]
fn test_unused_declared_import_is_not_in_the_module_hash() {
    let sdk = Sdk::default();
    let basics_src = r#"
        fn FindLog(action) {
            var log = action.output("Log");
        }
    "#;
    let basics = sdk
        .load_module_from_src_actions(basics_src, &["FindLog"], &[])
        .unwrap();

    let idle_src = r#"
        fn Idle(action) {
            var rock = action.output("Rock");
        }
    "#;
    let without = sdk
        .load_module_from_src_actions(idle_src, &["Idle"], &[])
        .unwrap();
    let with_unused = sdk
        .load_module_from_src_actions(
            idle_src,
            &["Idle"],
            &imports_of([("craft-basics", &basics)]),
        )
        .expect("an uncalled declared import loads");

    assert_eq!(
        without.module().batch.id(),
        with_unused.module().batch.id(),
        "an uncalled import contributes no predicate, so it cannot move the batch id"
    );
    assert!(
        with_unused.podlang_src().contains("as craft_basics"),
        "but it is still emitted, so the archive requires it at load:\n{}",
        with_unused.podlang_src()
    );
}

/// Two manifests importing each other fail resolution with a cycle
/// error before any pin validation runs.
#[test]
fn test_import_cycle_rejected() {
    let manifest = |name: &str, dep: &str| -> Manifest {
        toml::from_str(&format!(
            r#"
            [plugin]
            name = "{name}"
            version = "0.1.0"
            module_hash = "0000000000000000000000000000000000000000000000000000000000000000"

            [[imports]]
            name = "{dep}"
            module_hash = "0000000000000000000000000000000000000000000000000000000000000000"

            [[classes]]
            name = "Thing"
            emoji = "T"
            description = "a thing"

            [[actions]]
            name = "MakeThing"
            emoji = "T"
            description = "make a thing"
            "#
        ))
        .unwrap()
    };
    let ouro = manifest("ouro", "boros");
    let boros = manifest("boros", "ouro");
    let script = r#"
        fn MakeThing(action) {
            var thing = action.output("Thing");
        }
    "#;
    let sdk = Sdk::default();
    let mut resolver = ImportResolver::new(
        &sdk,
        [(&ouro, script), (&boros, script)],
        ImportLookup::DeclaredName,
    );
    let err = resolver.load_named("ouro").err().expect("cycle must fail");
    assert!(
        err.to_string().contains("import cycle"),
        "unexpected error: {err}"
    );
}
