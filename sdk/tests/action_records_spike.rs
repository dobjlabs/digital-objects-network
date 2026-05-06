//! Phase 0 / Test 1 of `docs/plans/action_records.md`.
//!
//! Validates that the records-based action shape compiles in pod2 ahead of
//! changing the SDK lowering. Hand-writes one action (`UseWoodPick`-shaped:
//! mutate with sub-field access), the per-(action,object) bridge predicate,
//! and the IsX OR, then asks `pod2::lang::load_module` to validate and lower
//! it under the workspace's pinned pod2.
//!
//! What this exercises:
//!
//! - Record schema declarations (`record UseWoodPickIn = (pick)`) and typed
//!   wildcards (`in UseWoodPickIn`).
//! - `ArrayContains(record_wc, <Schema>::<entry>, flat_wc)` boundary
//!   bridges, both on the action body and inside the IsX bridge predicate.
//! - 2-level-AK avoidance via the input bridge: `pick0.durability` works
//!   because `pick0` is a flat wildcard.
//! - AK-via-post-state-dict witness absorption: `pick1.durability` and
//!   `pick2.key` carry the new field values without separate witnesses.
//! - Cross-module call to `tx::TxMutate` with flat wildcards bound through
//!   the SSA chain.
//! - `@self_predicate(IsWoodPick)` resolves within the same batch.
//! - The bridge predicate composes inside the IsX OR.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use pod2::{
    backends::plonky2::{basetypes::DEFAULT_VD_SET, mock::mainpod::MockProver},
    frontend::{MainPodBuilder, MultiPodBuilder, Operation},
    lang::load_module,
    middleware::{
        EMPTY_VALUE, Hash, Params, Predicate, StrKey, VDSet, Value,
        containers::{Array, Dictionary, Set},
        hash_values,
    },
};
use pod2utils::macros::BuildContext;
use txlib::{GroundingWitness, StateRoot, Tx, TxBuilder};

/// Hand-written records-form podlang module. Mirrors the design in
/// `docs/plans/action_records.md`'s "Worked example" but drops the third
/// SSA step (the Vdf-driven `work` field) so the spike doesn't depend on
/// an intro pod module.
fn use_wood_pick_src(tx_hash: Hash) -> String {
    format!(
        r#"
use module {tx_hash:#} as tx

record UseWoodPickIn  = (pick)
record UseWoodPickOut = (pick)

UseWoodPick(in UseWoodPickIn, out UseWoodPickOut, chain0, chain,
    private: pick0, pick1, pick2) = AND(
  ArrayContains(in,  UseWoodPickIn::pick,  pick0)
  ArrayContains(out, UseWoodPickOut::pick, pick2)
  Gt(pick0.durability, 0)
  SumOf(pick0.durability, pick1.durability, 1)
  DictUpdate(pick1, pick0, "durability", pick1.durability)
  DictUpdate(pick2, pick1, "key",        pick2.key)
  DictContains(pick0, "type", @self_predicate(IsWoodPick))
  tx::TxMutate(chain, chain0, pick2, pick0)
)

IsWoodPickFromUseWoodPick(state, chain0, chain,
    private: in UseWoodPickIn, out UseWoodPickOut) = AND(
  ArrayContains(out, UseWoodPickOut::pick, state)
  UseWoodPick(in, out, chain0, chain)
)

IsWoodPick(state, chain0, chain) = OR(
  IsWoodPickFromUseWoodPick(state, chain0, chain)
)
"#,
    )
}

#[test]
fn records_form_compiles_under_default_params() {
    let txlib_mod = Arc::new(txlib::predicates::module());
    let src = use_wood_pick_src(txlib_mod.id());
    let params = Params::default();

    let module = match load_module(&src, "spike", &params, &[txlib_mod]) {
        Ok(m) => m,
        Err(e) => panic!(
            "records-form module failed to load:\n{}\n\nsource:\n{}",
            e, src
        ),
    };

    // The three named predicates exist. There may be additional auto-generated
    // helpers from the predicate splitter -- pod2 splits any predicate whose
    // statement count exceeds `Params::max_custom_predicate_arity()` (5 by
    // default) into a base + continuation chain. UseWoodPick has 8 sub-
    // statements, so it gets split. The user-visible name still resolves to
    // the final piece.
    let preds = module.batch.predicates();
    let names: Vec<&str> = preds.iter().map(|p| p.name.as_str()).collect();
    println!("predicate batch: {names:?}");
    for required in ["UseWoodPick", "IsWoodPickFromUseWoodPick", "IsWoodPick"] {
        assert!(
            names.contains(&required),
            "missing predicate {required:?} (have {names:?})"
        );
    }

    // The action predicate (final piece, post-split) fits the wildcard budget.
    // Pre-split it would be 7 wildcards; post-split each piece is even smaller,
    // since the splitter chooses cuts that minimize per-piece wildcard reuse.
    let use_wood_pick = module
        .predicate_ref_by_name("UseWoodPick")
        .expect("UseWoodPick is in the batch");
    let pred = use_wood_pick.predicate();
    let wc_count = pred.wildcard_names().len();

    println!(
        "UseWoodPick (final piece): {} wildcards (budget: {}), {} args public, {} statements",
        wc_count,
        params.max_custom_predicate_wildcards,
        pred.args_len(),
        pred.statements().len(),
    );

    assert!(
        wc_count <= params.max_custom_predicate_wildcards,
        "UseWoodPick has {} wildcards; budget is {}",
        wc_count,
        params.max_custom_predicate_wildcards
    );
    assert_eq!(pred.args_len(), 4, "in, out, chain0, chain are public");

    // Every split piece must be at-or-under the wildcard budget; surface them.
    for p in preds {
        let n = p.wildcard_names().len();
        println!(
            "  {}: {} wildcards, {} statements",
            p.name,
            n,
            p.statements().len()
        );
        assert!(
            n <= params.max_custom_predicate_wildcards,
            "predicate {} has {} wildcards; budget is {}",
            p.name,
            n,
            params.max_custom_predicate_wildcards
        );
    }

    // Bridge predicate: 3 public + 2 private = 5 wildcards (no split needed,
    // it's only 2 statements).
    let bridge = module
        .predicate_ref_by_name("IsWoodPickFromUseWoodPick")
        .expect("bridge predicate is in the batch");
    let bridge_pred = bridge.predicate();
    assert_eq!(bridge_pred.args_len(), 3, "state, chain0, chain are public");
    assert_eq!(
        bridge_pred.wildcard_names().len(),
        5,
        "state, chain0, chain + in, out"
    );

    // IsX OR: 3 public, no privates needed (the union of branches' privates
    // is empty since the branches are bridge calls with their own privates).
    let is_wood_pick = module
        .predicate_ref_by_name("IsWoodPick")
        .expect("IsWoodPick is in the batch");
    let is_wood_pick_pred = is_wood_pick.predicate();
    assert_eq!(is_wood_pick_pred.args_len(), 3, "state, chain0, chain");

    // Records were registered with the expected schemas.
    assert_eq!(
        module.records.get("UseWoodPickIn"),
        Some(&vec!["pick".to_string()]),
        "UseWoodPickIn schema",
    );
    assert_eq!(
        module.records.get("UseWoodPickOut"),
        Some(&vec!["pick".to_string()]),
        "UseWoodPickOut schema",
    );

    // `@self_predicate(IsWoodPick)` resolution: if it had failed, the
    // module would not have loaded. Recording a sanity assert on the hash
    // being computable for confidence.
    let _is_wood_pick_hash = Predicate::Custom(is_wood_pick).hash();
}

/// Phase 0 / Test 2 of `docs/plans/action_records.md`.
///
/// Discharges UseWoodPick end-to-end with `MockProver` against a concrete
/// (pre, post) pick dict pair and concrete `in`/`out` record arrays. This
/// is the actual unknown the spike validates: do the AK-via-post-state-dict
/// patterns and `ArrayContains` boundary bridges actually compose through
/// pod2's operation machinery at proof time?
#[test]
fn records_form_discharges_with_mock_prover() {
    let txlib_mod = Arc::new(txlib::predicates::module());
    let src = use_wood_pick_src(txlib_mod.id());
    let params = Params::default();
    let module = Arc::new(
        load_module(&src, "spike", &params, &[txlib_mod.clone()])
            .expect("module compiles (covered by Test 1)"),
    );

    // Hash of the IsWoodPick predicate. Used to stamp the pre/post pick
    // dicts' "type" field so the action's `DictContains(.., "type",
    // @self_predicate(IsWoodPick))` clause can be discharged.
    let is_wood_pick_ref = module
        .predicate_ref_by_name("IsWoodPick")
        .expect("IsWoodPick is in the batch");
    let is_wood_pick_hash = Predicate::Custom(is_wood_pick_ref).hash();

    // ---- Build the concrete dicts and record arrays ----------------------
    //
    // Pre-mutation pick: durability = 10, key = 42, type = IsWoodPick hash.
    // Post-mutation pick: durability = 9 (after SumOf), key = 99 (rolled),
    // type unchanged.
    //
    // The intermediate dict (between the two DictUpdates) bridges them:
    // it carries the new durability but the old key.
    let pre_pick_kvs: HashMap<StrKey, Value> = [
        (StrKey::from("type"), Value::from(is_wood_pick_hash)),
        (StrKey::from("key"), Value::from(42_i64)),
        (StrKey::from("durability"), Value::from(10_i64)),
    ]
    .into_iter()
    .collect();
    let pre_pick = pod2::middleware::containers::Dictionary::new(pre_pick_kvs);

    let intermediate_kvs: HashMap<StrKey, Value> = [
        (StrKey::from("type"), Value::from(is_wood_pick_hash)),
        (StrKey::from("key"), Value::from(42_i64)),
        (StrKey::from("durability"), Value::from(9_i64)),
    ]
    .into_iter()
    .collect();
    let intermediate = pod2::middleware::containers::Dictionary::new(intermediate_kvs);

    let post_pick_kvs: HashMap<StrKey, Value> = [
        (StrKey::from("type"), Value::from(is_wood_pick_hash)),
        (StrKey::from("key"), Value::from(99_i64)),
        (StrKey::from("durability"), Value::from(9_i64)),
    ]
    .into_iter()
    .collect();
    let post_pick = pod2::middleware::containers::Dictionary::new(post_pick_kvs);

    // Single-entry record arrays. UseWoodPickIn / UseWoodPickOut both have
    // schema (pick), so "pick" lives at index 0.
    assert_eq!(module.records["UseWoodPickIn"], vec!["pick".to_string()]);
    let in_array = Array::new(vec![Value::from(pre_pick.clone())]);
    let out_array = Array::new(vec![Value::from(post_pick.clone())]);

    // ---- Set up the prover ----------------------------------------------
    //
    // MockProver works with an empty VDSet. Real Prover would need
    // DEFAULT_VD_SET, but that's slow and we only need correctness here.
    let vd_set = VDSet::new(&[]);
    let _ = &*DEFAULT_VD_SET; // touch so the lazy_static doesn't get warned-about
    let mut builder = MainPodBuilder::new(&params, &vd_set);

    // ---- Discharge each clause of UseWoodPick's body --------------------
    //
    // We feed `apply_predicate` the per-clause Statements in the
    // declaration order from the source. Each Statement is built from an
    // Operation that produces it.
    //
    // Where the template arg is an AK like `pick0.durability`, the prover
    // passes `(&dict, "key")` -- this lowers to a `Contains` statement
    // that the operation machinery rewrites into the AK form.

    // 1) ArrayContains(in,  UseWoodPickIn::pick,  pick0)
    let st_in_contains = builder
        .priv_op(Operation::array_contains(
            Value::from(in_array.clone()),
            0_i64,
            Value::from(pre_pick.clone()),
        ))
        .expect("in array_contains");

    // 2) ArrayContains(out, UseWoodPickOut::pick, pick2)
    let st_out_contains = builder
        .priv_op(Operation::array_contains(
            Value::from(out_array.clone()),
            0_i64,
            Value::from(post_pick.clone()),
        ))
        .expect("out array_contains");

    // 3) Gt(pick0.durability, 0)
    let st_gt = builder
        .priv_op(Operation::gt((&pre_pick, "durability"), 0_i64))
        .expect("gt durability");

    // 4) SumOf(pick0.durability, pick1.durability, 1)
    //    The SumOf semantics: arg0 = arg1 + arg2 (in this case
    //    pre_pick.durability == intermediate.durability + 1, i.e. 10 == 9 + 1).
    let st_sum_of = builder
        .priv_op(Operation::sum_of(
            (&pre_pick, "durability"),
            (&intermediate, "durability"),
            1_i64,
        ))
        .expect("sum_of durability");

    // 5) DictUpdate(pick1, pick0, "durability", pick1.durability)
    let st_update_dur = builder
        .priv_op(Operation::dict_update(
            Value::from(intermediate.clone()),
            Value::from(pre_pick.clone()),
            "durability",
            (&intermediate, "durability"),
        ))
        .expect("update durability");

    // 6) DictUpdate(pick2, pick1, "key", pick2.key)
    let st_update_key = builder
        .priv_op(Operation::dict_update(
            Value::from(post_pick.clone()),
            Value::from(intermediate.clone()),
            "key",
            (&post_pick, "key"),
        ))
        .expect("update key");

    // 7) DictContains(pick0, "type", @self_predicate(IsWoodPick))
    let st_type_guard = builder
        .priv_op(Operation::dict_contains(
            Value::from(pre_pick.clone()),
            "type",
            Value::from(is_wood_pick_hash),
        ))
        .expect("type guard");

    // 8) tx::TxMutate(chain, chain0, pick2, pick0)
    //    TxMutate's body is `HashOf(event_hash, old, new); HashOf(chain, prev_chain, event_hash)`.
    //    Synthesise concrete chain values and discharge both HashOf clauses,
    //    then apply_predicate the TxMutate predicate.
    let chain0 = Hash::default(); // arbitrary; the spike doesn't care which
    let event_hash = hash_values(&[
        Value::from(pre_pick.clone()),
        Value::from(post_pick.clone()),
    ]);
    let chain = hash_values(&[Value::from(chain0), Value::from(event_hash)]);

    let st_h1 = builder
        .priv_op(Operation::hash_of(
            Value::from(event_hash),
            Value::from(pre_pick.clone()),
            Value::from(post_pick.clone()),
        ))
        .expect("hash_of event");
    let st_h2 = builder
        .priv_op(Operation::hash_of(
            Value::from(chain),
            Value::from(chain0),
            Value::from(event_hash),
        ))
        .expect("hash_of chain");

    let st_tx_mutate = txlib_mod
        .apply_predicate(&mut builder, "TxMutate", vec![st_h1, st_h2], false)
        .expect("apply TxMutate");

    // ---- Discharge the action predicate ----------------------------------
    let st_action = module
        .apply_predicate(
            &mut builder,
            "UseWoodPick",
            vec![
                st_in_contains,
                st_out_contains,
                st_gt,
                st_sum_of,
                st_update_dur,
                st_update_key,
                st_type_guard,
                st_tx_mutate,
            ],
            true,
        )
        .expect("apply UseWoodPick");

    println!("UseWoodPick discharged: {st_action}");

    // ---- Prove with MockProver -------------------------------------------
    let prover = MockProver {};
    let pod = builder.prove(&prover).expect("MockProver");
    pod.pod.verify().expect("verify");
}

/// Phase 0 / Test 3 of `docs/plans/action_records.md`.
///
/// Drives a full `TxFinalized` proof through `txlib::TxBuilder` using the
/// records-form `UseWoodPick`, with replay-time guard dispatch resolving
/// to `IsWoodPickFromUseWoodPick`. Validates that the bridge-predicate IsX
/// composition matches what `ReplayMutate`'s
/// `guard(new, before_tx.chain_start, before_tx.chain_end)` clause
/// expects, and that the post-mutation pick's `"type"` field correctly
/// pins to the records-form `IsWoodPick` hash.
///
/// Skipping the SpawnWoodPick step: we synthesize a "source transaction"
/// directly as a `Tx` struct whose live set contains `pre_pick`, register
/// it in the test state's transactions tree, and feed the merkle proof
/// through `GroundingWitness`. This is enough for `TxInStateRoot` to
/// discharge -- the only thing it checks is that the source tx's
/// commitment is in `state_root.transactions_root`.
#[test]
fn records_form_replays_through_tx_finalized() {
    let txlib_mod = Arc::new(txlib::predicates::module());
    let src = use_wood_pick_src(txlib_mod.id());
    let params = Params::default();
    let module = Arc::new(
        load_module(&src, "spike", &params, &[txlib_mod.clone()]).expect("module compiles"),
    );

    let is_wood_pick_hash = Predicate::Custom(
        module
            .predicate_ref_by_name("IsWoodPick")
            .expect("IsWoodPick exists"),
    )
    .hash();

    // ---- Build the dicts ------------------------------------------------
    let pre_pick = Dictionary::new(
        [
            (StrKey::from("type"), Value::from(is_wood_pick_hash)),
            (StrKey::from("key"), Value::from(42_i64)),
            (StrKey::from("durability"), Value::from(10_i64)),
        ]
        .into_iter()
        .collect::<HashMap<_, _>>(),
    );
    let intermediate = Dictionary::new(
        [
            (StrKey::from("type"), Value::from(is_wood_pick_hash)),
            (StrKey::from("key"), Value::from(42_i64)),
            (StrKey::from("durability"), Value::from(9_i64)),
        ]
        .into_iter()
        .collect::<HashMap<_, _>>(),
    );
    let post_pick = Dictionary::new(
        [
            (StrKey::from("type"), Value::from(is_wood_pick_hash)),
            (StrKey::from("key"), Value::from(99_i64)),
            (StrKey::from("durability"), Value::from(9_i64)),
        ]
        .into_iter()
        .collect::<HashMap<_, _>>(),
    );

    let in_array = Array::new(vec![Value::from(pre_pick.clone())]);
    let out_array = Array::new(vec![Value::from(post_pick.clone())]);

    // ---- Synthesize the source transaction ------------------------------
    //
    // A real production source-tx would have been produced by some prior
    // SpawnWoodPick action. We just need a Tx struct whose ctx commitment
    // sits in a transactions tree we can prove against.
    let zero: Hash = EMPTY_VALUE.into();
    let mut source_tx_live: Set = Set::new(HashSet::new());
    source_tx_live
        .insert(&Value::from(pre_pick.clone()))
        .expect("insert pre_pick into source tx live set");
    let empty_set = Set::new(HashSet::new());

    let source_tx_ctx = Dictionary::new(
        [
            (StrKey::from("live"), Value::from(source_tx_live.clone())),
            (StrKey::from("nullifiers"), Value::from(empty_set.clone())),
            (StrKey::from("chain_start"), Value::from(zero)),
            (StrKey::from("chain_end"), Value::from(zero)),
        ]
        .into_iter()
        .collect::<HashMap<_, _>>(),
    );

    // ---- Build the test state root --------------------------------------
    let mut transactions: Set = Set::new(HashSet::new());
    transactions
        .insert(&Value::from(source_tx_ctx.clone()))
        .expect("insert source tx into state's transactions tree");
    let state_root = StateRoot::new(0, transactions.commitment(), zero, zero);

    let source_tx_proof = transactions
        .prove(&Value::from(source_tx_ctx.clone()))
        .expect("prove source_tx in transactions");
    let mut source_tx_proofs = HashMap::new();
    source_tx_proofs.insert(source_tx_ctx.commitment(), source_tx_proof);
    let grounding = Arc::new(GroundingWitness::new(state_root.clone(), source_tx_proofs));

    let source_tx = Tx {
        live: source_tx_live,
        nullifiers: empty_set,
        ctx: source_tx_ctx,
        state_root: Arc::new(state_root),
    };

    // ---- Set up the build context ---------------------------------------
    let vd_set = VDSet::new(&[]);
    let _ = &*DEFAULT_VD_SET;
    let multi_builder = MultiPodBuilder::new(&params, &vd_set);
    let mut bld = BuildContext {
        builder: multi_builder,
        modules: vec![txlib_mod.clone(), module.clone()],
    };

    let inputs = vec![(pre_pick.clone(), source_tx)];
    let mut tx_builder = TxBuilder::new(&mut bld, &inputs, grounding);

    // ---- Open action scope and emit the mutate event --------------------
    let scope_id = tx_builder.begin_action();

    // tx_builder.mutate handles HashOf + TxMutate internally. Returns the
    // TxMutate statement that we feed into UseWoodPick.
    let (st_tx_mutate, handle) = tx_builder.mutate(&mut bld, &post_pick, &pre_pick);

    // ---- Discharge UseWoodPick's other 7 sub-statements -----------------
    let st_in_contains = bld
        .builder
        .priv_op(Operation::array_contains(
            Value::from(in_array.clone()),
            0_i64,
            Value::from(pre_pick.clone()),
        ))
        .expect("in array_contains");
    let st_out_contains = bld
        .builder
        .priv_op(Operation::array_contains(
            Value::from(out_array.clone()),
            0_i64,
            Value::from(post_pick.clone()),
        ))
        .expect("out array_contains");
    let st_gt = bld
        .builder
        .priv_op(Operation::gt((&pre_pick, "durability"), 0_i64))
        .expect("gt");
    let st_sum_of = bld
        .builder
        .priv_op(Operation::sum_of(
            (&pre_pick, "durability"),
            (&intermediate, "durability"),
            1_i64,
        ))
        .expect("sum_of");
    let st_update_dur = bld
        .builder
        .priv_op(Operation::dict_update(
            Value::from(intermediate.clone()),
            Value::from(pre_pick.clone()),
            "durability",
            (&intermediate, "durability"),
        ))
        .expect("dict_update durability");
    let st_update_key = bld
        .builder
        .priv_op(Operation::dict_update(
            Value::from(post_pick.clone()),
            Value::from(intermediate.clone()),
            "key",
            (&post_pick, "key"),
        ))
        .expect("dict_update key");
    let st_type_guard = bld
        .builder
        .priv_op(Operation::dict_contains(
            Value::from(pre_pick.clone()),
            "type",
            Value::from(is_wood_pick_hash),
        ))
        .expect("type guard");

    // ---- Discharge UseWoodPick ------------------------------------------
    let st_action = bld
        .apply_custom_pred_simple(
            false,
            "UseWoodPick",
            vec![
                st_in_contains,
                st_out_contains,
                st_gt,
                st_sum_of,
                st_update_dur,
                st_update_key,
                st_type_guard,
                st_tx_mutate,
            ],
        )
        .expect("apply UseWoodPick");

    // ---- Discharge bridge predicate -------------------------------------
    let st_bridge_ac = bld
        .builder
        .priv_op(Operation::array_contains(
            Value::from(out_array.clone()),
            0_i64,
            Value::from(post_pick.clone()),
        ))
        .expect("bridge array_contains");
    let st_bridge = bld
        .apply_custom_pred_simple(
            false,
            "IsWoodPickFromUseWoodPick",
            vec![st_bridge_ac, st_action],
        )
        .expect("apply bridge");

    // ---- Discharge IsWoodPick OR (1 branch) -----------------------------
    let st_is_wood_pick = bld
        .apply_custom_pred_simple(false, "IsWoodPick", vec![st_bridge])
        .expect("apply IsWoodPick");

    // ---- Attach as guard, close scope, finalize -------------------------
    tx_builder.set_guard(handle, st_is_wood_pick);
    tx_builder.end_action(scope_id);

    let (st_tx_finalized, _tx, _stats) = tx_builder.finalize(&mut bld);
    bld.builder
        .reveal(&st_tx_finalized)
        .expect("reveal TxFinalized");

    // ---- Solve and prove ------------------------------------------------
    let solution = bld.builder.solve().expect("solve");
    let pod = solution
        .prove(&MockProver {})
        .expect("MockProver")
        .pods
        .pop()
        .expect("at least one pod");
    pod.pod.verify().expect("verify");

    println!("TxFinalized verified: {}", st_tx_finalized);
}
