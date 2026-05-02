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
            log.set([["blueprint", "Log"]]);
            var work = action.intro_vdf(3, log);
            log.update("work", work);
        }

        fn CraftWood(action) {
            var log = action.input("Log");
            var wood = action.output("Wood");
            wood.set([["blueprint", "Wood"]]);
            let target = action.top_limb_u256(9007199254740992);
            var key = action.pow_obj_grind(wood, target);
            wood.update("key", key);
            action.intro_lt_eq_u256(wood, target);
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
            stone.set([["blueprint", "Stone"]]);
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
    assert_eq!(classes(action.inputs()), classes(action.total_inputs()));
    assert_eq!(classes(action.inputs()), Vec::<&str>::new());
    assert_eq!(classes(action.outputs()), classes(action.total_outputs()));
    assert_eq!(classes(action.outputs()), vec!["Log"]);
    // CraftWood
    let action = &actions[1];
    assert_eq!(classes(action.inputs()), classes(action.total_inputs()));
    assert_eq!(classes(action.inputs()), vec!["Log"]);
    assert_eq!(classes(action.outputs()), classes(action.total_outputs()));
    assert_eq!(classes(action.outputs()), vec!["Wood"]);
    // CraftSticks
    let action = &actions[2];
    assert_eq!(classes(action.inputs()), classes(action.total_inputs()));
    assert_eq!(classes(action.inputs()), vec!["Wood"]);
    assert_eq!(classes(action.outputs()), classes(action.total_outputs()));
    assert_eq!(classes(action.outputs()), vec!["Stick", "Stick"]);
    // CraftWoodPick
    let action = &actions[3];
    assert_eq!(classes(action.inputs()), classes(action.total_inputs()));
    assert_eq!(classes(action.inputs()), vec!["Wood", "Stick"]);
    assert_eq!(classes(action.outputs()), classes(action.total_outputs()));
    assert_eq!(classes(action.outputs()), vec!["WoodPick"]);
    // UseWoodPick
    let action = &actions[4];
    assert_eq!(classes(action.inputs()), classes(action.total_inputs()));
    assert_eq!(classes(action.inputs()), vec!["WoodPick"]);
    assert_eq!(classes(action.outputs()), classes(action.total_outputs()));
    assert_eq!(classes(action.outputs()), vec!["WoodPick"]);
    // MineStoneWithWoodPick
    let action = &actions[5];
    assert_eq!(classes(action.inputs()), Vec::<&str>::new());
    assert_eq!(classes(action.total_inputs()), vec!["WoodPick"]);
    assert_eq!(classes(action.outputs()), vec!["Stone"]);
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

/// Round-trip a `signed_by` assertion: load a script that asserts
/// `SignedBy(subsidy, GOVT_PK)`, register the matching SecretKey on the
/// executor, run the action, and confirm the produced spendable's pod
/// verifies. Exercises the new Rhai bindings end-to-end against the
/// mock prover.
#[test]
fn test_signed_by() {
    use pod2::middleware::SecretKey;

    let sk = SecretKey::new_rand();
    let pk = sk.public_key();
    let pk_b58 = format!("{}", pk);

    let src = format!(
        r#"
        fn IssueSubsidy(action) {{
            var subsidy = action.output("Subsidy");
            subsidy.set([
                ["blueprint", "Subsidy"],
                ["max_bags", 5],
                ["bags_remaining", 5]
            ]);
            let GOVT_PK = action.public_key("{pk_b58}");
            action.signed_by(subsidy, GOVT_PK);
        }}
"#
    );

    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(&src, &["IssueSubsidy"])
        .unwrap();
    println!("{}", module.podlang_src);
    assert!(
        module.podlang_src.contains("SignedBy(subsidy, PublicKey("),
        "rendered podlang missing SignedBy clause:\n{}",
        module.podlang_src
    );

    let state = TestState::default();
    let mut executor = module.executor(true, grounding_witness(&state, &[]));
    executor.add_signer(sk);
    let [subsidy] = executor
        .action("IssueSubsidy", vec![])
        .unwrap()
        .objs();
    subsidy.pod.pod.verify().unwrap();
}

/// One-shot helper: generates two keypairs and prints them so the
/// subsidy-demo plugin can embed the public keys and the test fixtures
/// can hold the secret keys. Run with:
///
/// ```text
/// cargo test -p sdk --release --lib gen_demo_keys -- --ignored --nocapture
/// ```
#[test]
#[ignore]
fn gen_demo_keys() {
    use pod2::middleware::SecretKey;
    for label in ["EMPLOYER", "GOVT"] {
        let sk = SecretKey::new_rand();
        let pk_b58 = format!("{}", sk.public_key());
        let sk_json = serde_json::to_string(&sk).unwrap();
        println!("{label}_PK_B58 = {pk_b58}");
        println!("{label}_SK_JSON = {sk_json}");
    }
}

/// End-to-end subsidy-issuance flow exercising both new bindings:
/// - employer issues a `SignedDict` income credential off-band
/// - the action consumes it via `input_signed_dict(EMPLOYER_PK)`
/// - the action mints a Subsidy and signs it via `signed_by(subsidy, GOVT_PK)`
///
/// Mirrors the structure of `IssueSubsidy` from the subsidy demo plan
/// and confirms a single action can verify an external credential and
/// produce a freshly-signed object in one shot.
#[test]
fn test_input_signed_dict_issue_subsidy() {
    use pod2::{
        frontend::SignedDictBuilder,
        middleware::{Params, SecretKey},
    };
    use pod2::backends::plonky2::signer::Signer as PodSigner;

    let employer_sk = SecretKey::new_rand();
    let employer_pk_b58 = format!("{}", employer_sk.public_key());
    let govt_sk = SecretKey::new_rand();
    let govt_pk_b58 = format!("{}", govt_sk.public_key());

    let src = format!(
        r#"
        fn IssueSubsidy(action) {{
            let EMPLOYER_PK = action.public_key("{employer_pk_b58}");
            let GOVT_PK = action.public_key("{govt_pk_b58}");
            var income = action.input_signed_dict(EMPLOYER_PK);
            var subsidy = action.output("Subsidy");
            subsidy.set([
                ["blueprint", "Subsidy"],
                ["max_bags", 5],
                ["bags_remaining", 5]
            ]);
            action.signed_by(subsidy, GOVT_PK);
        }}
"#
    );

    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(&src, &["IssueSubsidy"])
        .unwrap();
    println!("{}", module.podlang_src);
    assert!(module.podlang_src.contains("SignedBy(income,"));
    assert!(module.podlang_src.contains("SignedBy(subsidy,"));

    // Employer signs an income credential off-band.
    let params = Params::default();
    let mut income = SignedDictBuilder::new(&params);
    income.insert("income", 30000_i64);
    income.insert("year", 2026_i64);
    let income_signed = income.sign(&PodSigner(employer_sk)).unwrap();

    let state = TestState::default();
    let mut executor = module.executor(true, grounding_witness(&state, &[]));
    executor.add_signer(govt_sk);
    executor.add_signed_input(income_signed);

    let [subsidy] = executor
        .action("IssueSubsidy", vec![])
        .unwrap()
        .objs();
    subsidy.pod.pod.verify().unwrap();
}

/// Black-box demo flow: load the on-disk `plugins/subsidy-demo` plugin
/// (manifest + script), validate it against the committed module_hash,
/// and walk through the full subsidy lifecycle:
///
///   1. Employer signs an income credential (off-chain).
///   2. Govt runs `IssueSubsidy` → Subsidy with 5 bags.
///   3. Farmer runs `RedeemGrainBag` 3× → Subsidy(2 bags) + 3 Grains.
///   4. Merchant runs `ConsumeGrain` on the first Grain (receipt).
///   5. Drain remaining 2 bags; the 6th `RedeemGrainBag` must fail at
///      `Gt(bags_remaining, 0)`.
///
/// Uses the demo-keypair fixtures (matching pubkeys are baked into
/// `plugin.rhai`).
#[test]
fn test_subsidy_demo_plugin_e2e() {
    use pod2::{
        backends::plonky2::signer::Signer as PodSigner,
        frontend::SignedDictBuilder,
        middleware::{Params, SecretKey},
    };
    use std::path::PathBuf;

    let plugin_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("plugins/subsidy-demo");
    let manifest_toml = std::fs::read_to_string(plugin_dir.join("manifest.toml")).unwrap();
    let script = std::fs::read_to_string(plugin_dir.join("plugin.rhai")).unwrap();
    let manifest: Manifest = toml::from_str(&manifest_toml).unwrap();

    // Demo SKs (matching pubkeys are committed into plugin.rhai). Regenerate via
    // `cargo test -p sdk gen_demo_keys -- --ignored --nocapture`.
    let employer_sk: SecretKey =
        serde_json::from_str(r#""Y3+yrQr+17yQqSZWTxPmyZNXDFzxIk45kX2xHA52QCQ9jAj39Jigfw==""#)
            .unwrap();
    let govt_sk: SecretKey =
        serde_json::from_str(r#""XSi0uGplfaoiH1g47EyR6YMZRFd8VE8OhMo7HB23TXn3othmbneoEA==""#)
            .unwrap();

    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_manifest(&script, &manifest)
        .unwrap();
    println!("{}", module.podlang_src);

    // 1. Employer signs an income credential off-band.
    let params = Params::default();
    let mut income = SignedDictBuilder::new(&params);
    income.insert("income", 30000_i64);
    income.insert("year", 2026_i64);
    let income_signed = income.sign(&PodSigner(employer_sk)).unwrap();

    let mut state = TestState::default();

    // 2. Govt issues the subsidy.
    let mut executor = module.executor(true, grounding_witness(&state, &[]));
    executor.add_signer(govt_sk);
    executor.add_signed_input(income_signed);
    let [subsidy] = executor.action("IssueSubsidy", vec![]).unwrap().objs();
    apply_tx(&mut state, &subsidy.tx);
    assert_eq!(
        subsidy
            .obj
            .get(&"bags_remaining".into())
            .unwrap()
            .unwrap()
            .as_int()
            .unwrap(),
        5
    );

    // 3. Farmer redeems 3 bags. Each call mutates the subsidy and
    //    produces a Grain. Carry the mutated subsidy forward.
    let mut subsidy = subsidy;
    let mut grains = Vec::new();
    for expected_remaining in [4, 3, 2] {
        let executor = module.executor(true, grounding_witness(&state, &[subsidy.tx.clone()]));
        let [next_subsidy, grain] = executor
            .action("RedeemGrainBag", vec![subsidy.clone()])
            .unwrap()
            .objs();
        apply_tx(&mut state, &next_subsidy.tx);
        assert_eq!(
            next_subsidy
                .obj
                .get(&"bags_remaining".into())
                .unwrap()
                .unwrap()
                .as_int()
                .unwrap(),
            expected_remaining
        );
        subsidy = next_subsidy;
        grains.push(grain);
    }

    // 4. Merchant consumes the first grain (receipt).
    let first_grain = grains.remove(0);
    let executor = module.executor(true, grounding_witness(&state, &[first_grain.tx.clone()]));
    let _ = executor.action("ConsumeGrain", vec![first_grain]).unwrap();

    // 5. Drain remaining 2 bags, then verify the 6th call fails.
    for expected_remaining in [1, 0] {
        let executor = module.executor(true, grounding_witness(&state, &[subsidy.tx.clone()]));
        let [next_subsidy, _grain] = executor
            .action("RedeemGrainBag", vec![subsidy.clone()])
            .unwrap()
            .objs();
        apply_tx(&mut state, &next_subsidy.tx);
        assert_eq!(
            next_subsidy
                .obj
                .get(&"bags_remaining".into())
                .unwrap()
                .unwrap()
                .as_int()
                .unwrap(),
            expected_remaining
        );
        subsidy = next_subsidy;
    }
    let executor = module.executor(true, grounding_witness(&state, &[subsidy.tx.clone()]));
    let exhausted = executor.action("RedeemGrainBag", vec![subsidy]);
    assert!(
        exhausted.is_err(),
        "RedeemGrainBag should fail once bags_remaining hits 0"
    );
}

/// Negative case: if no signer is registered for the script's public
/// key, `signed_by` must fail with a clear error rather than panicking
/// or emitting a malformed pod.
#[test]
fn test_signed_by_missing_signer() {
    use pod2::middleware::SecretKey;

    let sk = SecretKey::new_rand();
    let pk_b58 = format!("{}", sk.public_key());

    let src = format!(
        r#"
        fn IssueSubsidy(action) {{
            var subsidy = action.output("Subsidy");
            subsidy.set([["blueprint", "Subsidy"]]);
            let GOVT_PK = action.public_key("{pk_b58}");
            action.signed_by(subsidy, GOVT_PK);
        }}
"#
    );

    let sdk = Sdk::default();
    let module = sdk
        .load_module_from_src_actions(&src, &["IssueSubsidy"])
        .unwrap();

    let state = TestState::default();
    // Don't register any signer — execution should fail at signed_by.
    let executor = module.executor(true, grounding_witness(&state, &[]));
    let err = executor
        .action("IssueSubsidy", vec![])
        .err()
        .expect("expected signed_by to fail without a registered signer");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("no signer registered"),
        "unexpected error: {msg}"
    );
}

#[allow(clippy::cloned_ref_to_slice_refs)]
#[test]
fn test_sdk_2() {
    let manifest_src = r#"
        [plugin]
        name = "test"
        version = "0.1.0"
        module_hash = "89186d51b500e63c74bc8b797f2f9268ed9e883f6c5525138bfd3f4cc6ba4cf6"

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
            log.set([["blueprint", "Log"]]);
            var work = action.intro_vdf(3, log);
            log.update("work", work);
        }

        fn CraftWood(action) {
            var log = action.input("Log");
            var wood = action.output("Wood");
            wood.set([["blueprint", "Wood"]]);
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
