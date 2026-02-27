use std::sync::Arc;

use pod2::lang::{self, load_module};

use crate::predicates;

const TXLIB_HASH_PLACEHOLDER: &str = "0xTXLIB_MODULE_HASH";

pub fn mine_ore_module() -> Result<lang::Module, lang::LangError> {
    log::info!("Loading mine_ore example predicates");
    let params = pod2::middleware::Params::default();

    let txlib_module = Arc::new(predicates::module()?);
    let txlib_hash = format!("{:#}", txlib_module.id());
    let source = include_str!("mine_ore.podlang").replace(TXLIB_HASH_PLACEHOLDER, &txlib_hash);

    load_module(&source, "txlib_examples_mine_ore", &params, &[txlib_module])
}

#[cfg(test)]
mod tests {

    use pod2::{
        backends::plonky2::mock::mainpod::MockProver,
        dict,
        frontend::MultiPodBuilder,
        middleware::{EMPTY_VALUE, Params, Statement, VDSet, Value, hash_values},
    };
    use pod2utils::{macros::BuildContext, op, pub_st_custom, set, st_custom};

    use super::*;

    #[test]
    fn test_example_predicates_exist() {
        let module = mine_ore_module().unwrap();
        module.predicate_ref_by_name("Pickaxe").unwrap();
        module.predicate_ref_by_name("Pickaxe_new").unwrap();
        module.predicate_ref_by_name("MineOre_transition").unwrap();
        module.predicate_ref_by_name("MineOre").unwrap();
    }

    #[test]
    fn prove_mine_ore_transaction_finalized() {
        let txlib_module = predicates::module().unwrap();
        let example_module = mine_ore_module().unwrap();
        let empty_set = set!();
        let empty_dict = dict!({});
        let state_root_hash = hash_values(&[
            Value::from(empty_set.clone()),
            Value::from(empty_set.clone()),
        ]);

        let s1 = dict!({"live" => empty_set.clone()});
        let s2 = dict!({
            "live" => empty_set.clone(),
            "state_root_hash" => state_root_hash,
        });

        let pick_before = dict!({
            "material" => "stone",
            "durability" => 10_i64,
            "key" => 1001_i64,
        });
        let pick_after = dict!({
            "material" => "stone",
            "durability" => 9_i64,
            "key" => 1001_i64,
        });
        let ore_vein = dict!({
            "ore_type" => "iron",
            "richness" => 3_i64,
            "key" => 2001_i64,
        });
        let ore_chunk = dict!({
            "ore_type" => "iron",
            "amount" => 1_i64,
            "key" => 3001_i64,
        });

        let tx1_live = set![pick_before.clone()];
        let tx2_live = set![pick_before.clone(), ore_vein.clone()];
        let tx3_live = set![pick_before.clone()];
        let tx4_live = set![pick_after.clone()];
        let tx_final_live = set![pick_after.clone(), ore_chunk.clone()];
        let ore_vein_keyed = hash_values(&[Value::from(ore_vein.clone()), Value::from(2001_i64)]);
        let ore_vein_nullifier = hash_values(&[
            Value::from(ore_vein_keyed),
            Value::from("txlib-nullifier-v1"),
        ]);
        let pick_before_keyed =
            hash_values(&[Value::from(pick_before.clone()), Value::from(1001_i64)]);
        let pick_before_nullifier = hash_values(&[
            Value::from(pick_before_keyed),
            Value::from("txlib-nullifier-v1"),
        ]);
        let tx3_nullifiers = set![ore_vein_nullifier];
        let tx4_nullifiers = set![ore_vein_nullifier, pick_before_nullifier];
        let tx_final_nullifiers = tx4_nullifiers.clone();
        let live_mid = set!();
        let tx3_nullified = dict!({
            "live" => tx2_live.clone(),
            "nullifiers" => tx3_nullifiers.clone(),
            "state_root_hash" => state_root_hash,
        });
        let tx4_nullified = dict!({
            "live" => tx3_live.clone(),
            "nullifiers" => tx4_nullifiers.clone(),
            "state_root_hash" => state_root_hash,
        });

        let tx0 = dict!({
            "live" => empty_set.clone(),
            "nullifiers" => empty_set.clone(),
            "state_root_hash" => state_root_hash,
        });
        let tx1 = dict!({
            "live" => tx1_live.clone(),
            "nullifiers" => empty_set.clone(),
            "state_root_hash" => state_root_hash,
        });
        let tx2 = dict!({
            "live" => tx2_live.clone(),
            "nullifiers" => empty_set.clone(),
            "state_root_hash" => state_root_hash,
        });
        let tx3 = dict!({
            "live" => tx3_live.clone(),
            "nullifiers" => tx3_nullifiers.clone(),
            "state_root_hash" => state_root_hash,
        });
        let tx4 = dict!({
            "live" => tx4_live.clone(),
            "nullifiers" => tx4_nullifiers.clone(),
            "state_root_hash" => state_root_hash,
        });
        let tx_final = dict!({
            "live" => tx_final_live.clone(),
            "nullifiers" => tx_final_nullifiers.clone(),
            "state_root_hash" => state_root_hash,
        });

        let params = Params::default();
        let vd_set = VDSet::new(&[]);
        let mut builder = MultiPodBuilder::new(&params, &vd_set);
        {
            let ctx = BuildContext::new(&mut builder, &txlib_module.batch);

            // InputsGrounded({}, state_root_hash)
            let st_inputs_grounded_empty = st_custom!(ctx,
                InputsGrounded(state_root_hash=state_root_hash) = (
                    Equal(empty_set, EMPTY_VALUE),
                    Statement::None
                ))
            .unwrap();

            // TxnInit(tx0, {}, {}, state_root_hash)
            let st_tx0_init = st_custom!(ctx,
                TxnInit(state_root_hash=state_root_hash) = (
                    Equal(empty_dict, EMPTY_VALUE),
                    DictInsert(s1, empty_dict, "live", empty_set),
                    DictInsert(s2, s1, "state_root_hash", state_root_hash),
                    DictInsert(tx0, s2, "nullifiers", empty_set),
                    st_inputs_grounded_empty
                ))
            .unwrap();
            let st_tx0_txn = st_custom!(ctx,
                Txn() = (
                    st_tx0_init,
                    Statement::None,
                    Statement::None,
                    Statement::None
                ))
            .unwrap();

            // TxnInserted(tx1, tx0, pick_before)
            let st_tx0_live_contains = ctx
                .builder
                .priv_op(op!(DictContains(tx0, "live", empty_set)))
                .unwrap();
            let st_tx1_inserted = st_custom!(ctx,
                TxnInserted() = (
                    st_tx0_txn,
                    SetInsert(tx1_live, st_tx0_live_contains, pick_before),
                    DictUpdate(tx1, tx0, "live", tx1_live)
                ))
            .unwrap();
            let st_tx1_txn = st_custom!(ctx,
                Txn() = (
                    Statement::None,
                    Statement::None,
                    Statement::None,
                    st_tx1_inserted
                ))
            .unwrap();

            // TxnInserted(tx2, tx1, ore_vein)
            let st_tx1_live_contains = ctx
                .builder
                .priv_op(op!(DictContains(tx1, "live", tx1_live)))
                .unwrap();
            let st_tx2_inserted = st_custom!(ctx,
                TxnInserted() = (
                    st_tx1_txn,
                    SetInsert(tx2_live, st_tx1_live_contains, ore_vein),
                    DictUpdate(tx2, tx1, "live", tx2_live)
                ))
            .unwrap();
            let st_tx2_txn = st_custom!(ctx,
                Txn() = (
                    Statement::None,
                    Statement::None,
                    Statement::None,
                    st_tx2_inserted
                ))
            .unwrap();

            // TxnDeleted(tx3, tx2, ore_vein)
            let st_tx2_live_contains = ctx
                .builder
                .priv_op(op!(DictContains(tx2, "live", tx2_live)))
                .unwrap();
            let st_tx2_nullifiers_contains = ctx
                .builder
                .priv_op(op!(DictContains(tx2, "nullifiers", empty_set)))
                .unwrap();
            let _st_ore_vein_key_contains = ctx
                .builder
                .priv_op(op!(DictContains(ore_vein, "key", 2001_i64)))
                .unwrap();
            let st_ore_vein_txn_object_nullified = st_custom!(ctx,
                TxnObjectStateNullified() = (
                    HashOf(ore_vein_keyed, ore_vein, (&ore_vein, "key")),
                    HashOf(ore_vein_nullifier, ore_vein_keyed, "txlib-nullifier-v1"),
                    SetInsert(tx3_nullifiers, st_tx2_nullifiers_contains, ore_vein_nullifier),
                    DictUpdate(tx3_nullified, tx2, "nullifiers", tx3_nullifiers)
                ))
            .unwrap();
            let st_tx_deleted = st_custom!(ctx,
                TxnDeleted() = (
                    st_tx2_txn,
                    SetDelete(tx3_live, st_tx2_live_contains, ore_vein),
                    st_ore_vein_txn_object_nullified,
                    DictUpdate(tx3, tx3_nullified, "live", tx3_live)
                ))
            .unwrap();
            let st_tx3_txn = st_custom!(ctx,
                Txn() = (
                    Statement::None,
                    st_tx_deleted.clone(),
                    Statement::None,
                    Statement::None
                ))
            .unwrap();

            // TxnMutated(tx4, tx3, pick_before, pick_after)
            let st_tx3_live_contains = ctx
                .builder
                .priv_op(op!(DictContains(tx3, "live", tx3_live)))
                .unwrap();
            let st_tx3_nullifiers_contains = ctx
                .builder
                .priv_op(op!(DictContains(tx3, "nullifiers", tx3_nullifiers)))
                .unwrap();
            let _st_pick_before_key_contains = ctx
                .builder
                .priv_op(op!(DictContains(pick_before, "key", 1001_i64)))
                .unwrap();
            let st_pick_before_txn_object_nullified = st_custom!(ctx,
                TxnObjectStateNullified() = (
                    HashOf(pick_before_keyed, pick_before, (&pick_before, "key")),
                    HashOf(pick_before_nullifier, pick_before_keyed, "txlib-nullifier-v1"),
                    SetInsert(tx4_nullifiers, st_tx3_nullifiers_contains, pick_before_nullifier),
                    DictUpdate(tx4_nullified, tx3, "nullifiers", tx4_nullifiers)
                ))
            .unwrap();
            let st_tx_mutated = st_custom!(ctx,
                TxnMutated() = (
                    st_tx3_txn,
                    SetDelete(live_mid, st_tx3_live_contains, pick_before),
                    SetInsert(tx4_live, live_mid, pick_after),
                    st_pick_before_txn_object_nullified,
                    DictUpdate(tx4, tx4_nullified, "live", tx4_live)
                ))
            .unwrap();
            let st_tx4_txn = st_custom!(ctx,
                Txn() = (
                    Statement::None,
                    Statement::None,
                    st_tx_mutated.clone(),
                    Statement::None
                ))
            .unwrap();

            // TxnInserted(tx_final, tx4, ore_chunk)
            let st_tx4_live_contains = ctx
                .builder
                .priv_op(op!(DictContains(tx4, "live", tx4_live)))
                .unwrap();
            let st_tx_final_nullifiers_contains = ctx
                .builder
                .priv_op(op!(DictContains(
                    tx_final,
                    "nullifiers",
                    tx_final_nullifiers
                )))
                .unwrap();
            let st_tx_final_state_root_hash_contains = ctx
                .builder
                .priv_op(op!(DictContains(
                    tx_final,
                    "state_root_hash",
                    state_root_hash
                )))
                .unwrap();
            let st_tx_inserted = st_custom!(ctx,
                TxnInserted() = (
                    st_tx4_txn,
                    SetInsert(tx_final_live, st_tx4_live_contains, ore_chunk),
                    DictUpdate(tx_final, tx4, "live", tx_final_live)
                ))
            .unwrap();
            let st_txn = st_custom!(ctx,
                Txn() = (
                    Statement::None,
                    Statement::None,
                    Statement::None,
                    st_tx_inserted.clone()
                ))
            .unwrap();

            // Example object predicates + interaction predicate, loaded from examples module.
            {
                let ex_ctx = BuildContext::new(&mut *ctx.builder, &example_module.batch);
                let st_pickaxe_new_before = st_custom!(ex_ctx,
                    Pickaxe_new() = (
                        DictContains(pick_before, "material", "stone"),
                        DictContains(pick_before, "durability", 10_i64),
                        DictContains(pick_before, "key", 1001_i64),
                        NotEqual(10_i64, 0_i64)
                    ))
                .unwrap();
                let st_pickaxe_before = st_custom!(ex_ctx,
                    Pickaxe() = (
                        st_pickaxe_new_before,
                        Statement::None
                    ))
                .unwrap();
                let st_pickaxe_step = st_custom!(ex_ctx,
                    Pickaxe_step() = (
                        st_pickaxe_before,
                        DictContains(pick_before, "durability", 10_i64),
                        NotEqual(10_i64, 0_i64),
                        SumOf(10_i64, 9_i64, 1_i64),
                        DictUpdate(pick_after, pick_before, "durability", 9_i64)
                    ))
                .unwrap();
                let st_ore_vein_new = st_custom!(ex_ctx,
                    OreVein_new() = (
                        DictContains(ore_vein, "ore_type", "iron"),
                        DictContains(ore_vein, "richness", 3_i64),
                        DictContains(ore_vein, "key", 2001_i64),
                        NotEqual(3_i64, 0_i64)
                    ))
                .unwrap();
                let st_ore_chunk_new = st_custom!(ex_ctx,
                    OreChunk_new() = (
                        DictContains(ore_chunk, "ore_type", "iron"),
                        DictContains(ore_chunk, "amount", 1_i64),
                        DictContains(ore_chunk, "key", 3001_i64),
                        NotEqual(1_i64, 0_i64)
                    ))
                .unwrap();
                let st_mine_ore_objects = st_custom!(ex_ctx,
                    MineOre_objects() = (
                        st_pickaxe_step,
                        st_ore_vein_new,
                        st_ore_chunk_new,
                        DictContains(ore_vein, "ore_type", "iron"),
                        DictContains(ore_chunk, "ore_type", "iron")
                    ))
                .unwrap();
                let st_mine_ore_transition = st_custom!(ex_ctx,
                    MineOre_transition() = (
                        st_tx_deleted,
                        st_tx_mutated,
                        st_tx_inserted
                    ))
                .unwrap();
                let _st_mine_ore = st_custom!(ex_ctx,
                    MineOre() = (
                        st_mine_ore_objects,
                        st_mine_ore_transition
                    ))
                .unwrap();
            }

            // TxnFinalized(tx_final, tx_final_nullifiers, state_root_hash)
            let _st_txn_finalized = pub_st_custom!(ctx,
                TxnFinalized(state_root_hash=state_root_hash) = (
                    st_txn,
                    st_tx_final_nullifiers_contains,
                    st_tx_final_state_root_hash_contains
                ))
            .unwrap();
        }

        let pod = builder
            .solve()
            .unwrap()
            .prove(&MockProver {})
            .unwrap()
            .output_pod()
            .clone();
        pod.pod.verify().unwrap();

        assert_eq!(pod.public_statements.len(), 1);
        let st = &pod.public_statements[0];
        let args = st.args();
        assert_eq!(args.len(), 3);
        assert_eq!(
            args[0].literal().unwrap().raw(),
            Value::from(tx_final).raw()
        );
        assert_eq!(
            args[1].literal().unwrap().raw(),
            Value::from(tx_final_nullifiers).raw()
        );
        assert_eq!(
            args[2].literal().unwrap().raw(),
            Value::from(state_root_hash).raw()
        );
    }
}
