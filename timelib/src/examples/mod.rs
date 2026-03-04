use std::sync::Arc;

use pod2::lang::{self, load_module};

const TXLIB_HASH_PLACEHOLDER: &str = "0xTXLIB_MODULE_HASH";
const TIMELIB_HASH_PLACEHOLDER: &str = "0xTIMELIB_MODULE_HASH";

pub fn creature_module() -> Result<lang::Module, lang::LangError> {
    let params = pod2::middleware::Params::default();
    let txlib_module = Arc::new(txlib::predicates::module());
    let time_module = Arc::new(crate::predicates::module().unwrap());
    let txlib_hash = format!("{:#}", txlib_module.id());
    let time_hash = format!("{:#}", time_module.id());
    let source = include_str!("creature.podlang")
        .replace(TXLIB_HASH_PLACEHOLDER, &txlib_hash)
        .replace(TIMELIB_HASH_PLACEHOLDER, &time_hash);
    load_module(
        &source,
        "creature",
        &params,
        &[txlib_module, time_module],
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use pod2::{
        backends::plonky2::mock::mainpod::MockProver,
        frontend::MultiPodBuilder,
        middleware::{Params, Statement, Value, VDSet, containers::Array},
    };
    use pod2utils::{macros::BuildContext, op, set, st_custom};
    use txlib::{Object, StateRoot, TxBuilder};

    use super::*;
    use crate::tx_utils;

    #[test]
    fn test_creature_predicates_exist() {
        let module = creature_module().unwrap();
        module.predicate_ref_by_name("NewCreature").unwrap();
        module.predicate_ref_by_name("Feed").unwrap();
        module.predicate_ref_by_name("IsCreature").unwrap();
    }

    /// Full creature lifecycle: create at block 100, feed at block 200.
    ///
    /// **POD 1 (block 100):** A `NewCreature` transaction inserts the creature
    /// with `timeout_block: 400` (= 100 + 300). Produces `tx_create`.
    ///
    /// **Time passes:** GSR₂ is produced at block 200 (before the deadline of 400).
    /// It carries `tx_create` in its transaction set.
    ///
    /// **POD 2 (block 200):** A `Feed` transaction mutates the creature, updating
    /// `timeout_block` to 500 (= 200 + 300). To satisfy `IsCreature(old_state)`,
    /// the proof re-proves `NewCreature` inline in the same builder.
    #[test]
    fn prove_creature_lifecycle() {
        let txlib_module = Arc::new(txlib::predicates::module());
        let time_module = Arc::new(crate::predicates::module().unwrap());
        let creature_module = Arc::new(creature_module().unwrap());

        let gsr1_block = 100_i64;
        let gsr2_block = 200_i64; // before the deadline of 400
        let create_timeout = gsr1_block + 300; // 400
        let feed_timeout = gsr2_block + 300; // 500

        let params = Params::default();
        let vd_set = VDSet::new(&[]);

        // Template: just blueprint; NewCreature injects timeout_block.
        let blueprint_template = Object::new(HashMap::from([(
            "blueprint".to_string(),
            Value::from("creature"),
        )]));
        // Full creature state after creation.
        let mut creature_state = blueprint_template.clone();
        creature_state
            .app_layer
            .insert("timeout_block".to_string(), Value::from(create_timeout));
        // Fed creature state after feeding.
        let mut fed_creature = creature_state.clone();
        fed_creature
            .app_layer
            .insert("timeout_block".to_string(), Value::from(feed_timeout));

        // GSR₁: block 100, empty.
        let gsr1_sr = Arc::new(StateRoot {
            block_number: gsr1_block,
            transactions: set!(),
            nullifiers: set!(),
            gsrs: Array::new(vec![]),
        });

        // Helper: prove NewCreature sub-statements in ctx, inserting creature_state
        // into the given tx_builder. Returns (st_new_creature, st_tx_inserted).
        let prove_new_creature_stmts =
            |ctx: &mut BuildContext,
             tx_builder: &mut TxBuilder,
             gsr_sr: &Arc<StateRoot>,
             gsr_block: i64,
             create_timeout: i64| {
                let tx0 = tx_builder.tx.dict();
                let st_blueprint = ctx
                    .builder
                    .priv_op(op!(DictContains(
                        blueprint_template.dict(),
                        "blueprint",
                        "creature"
                    )))
                    .unwrap();
                let st_gsr = tx_utils::prove_state_root(ctx, gsr_sr);
                let st_block_num = st_custom!(
                    ctx,
                    BlockNumberForStateRoot(block_number = gsr_block) = (st_gsr)
                )
                .unwrap();
                let st_sr_hash = ctx
                    .builder
                    .priv_op(op!(DictContains(tx0, "state_root_hash", gsr_sr.hash())))
                    .unwrap();
                let st_current_block = st_custom!(
                    ctx,
                    CurrentBlockNumber(block_number = gsr_block) = (st_sr_hash, st_block_num)
                )
                .unwrap();
                let st_sum = ctx
                    .builder
                    .priv_op(op!(SumOf(create_timeout, gsr_block, 300_i64)))
                    .unwrap();
                let st_dict_insert = ctx
                    .builder
                    .priv_op(op!(DictInsert(
                        creature_state.dict(),
                        blueprint_template.dict(),
                        "timeout_block",
                        create_timeout
                    )))
                    .unwrap();
                let st_tx_inserted = tx_builder.insert(ctx, creature_state.clone());
                st_custom!(
                    ctx,
                    NewCreature() = (
                        st_blueprint,
                        st_current_block,
                        st_sum,
                        st_dict_insert,
                        st_tx_inserted
                    )
                )
                .unwrap()
            };

        // === POD 1: Create creature ===
        let (create_pod, tx_create) = {
            let mut builder = MultiPodBuilder::new(&params, &vd_set);
            let tx_create = {
                let mods = [
                    Arc::clone(&txlib_module),
                    Arc::clone(&time_module),
                    Arc::clone(&creature_module),
                ];
                let mut ctx = BuildContext::new(&mut builder, &mods);
                let mut tx_builder = TxBuilder::new(&mut ctx, &[], gsr1_sr.clone());
                prove_new_creature_stmts(
                    &mut ctx,
                    &mut tx_builder,
                    &gsr1_sr,
                    gsr1_block,
                    create_timeout,
                );
                let (st_finalized, tx_create) = tx_builder.finalize(&mut ctx);
                ctx.builder.reveal(&st_finalized).unwrap();
                tx_create
            };
            let pod = builder
                .solve()
                .unwrap()
                .prove(&MockProver {})
                .unwrap()
                .output_pod()
                .clone();
            (pod, tx_create)
        };

        // GSR₂: block 200, carries tx_create; gsrs = [gsr1_hash].
        let mut gsr2_txs = set!();
        gsr2_txs
            .insert(&Value::from(tx_create.dict()))
            .unwrap();
        let gsr2_nullifiers = tx_create.nullifiers.clone();
        let gsr2_sr = Arc::new(StateRoot {
            block_number: gsr2_block,
            transactions: gsr2_txs,
            nullifiers: gsr2_nullifiers,
            gsrs: Array::new(vec![Value::from(gsr1_sr.hash())]),
        });

        // === POD 2: Feed creature ===
        let feed_pod = {
            let mut builder = MultiPodBuilder::new(&params, &vd_set);
            {
                let mods = [
                    Arc::clone(&txlib_module),
                    Arc::clone(&time_module),
                    Arc::clone(&creature_module),
                ];
                let mut ctx = BuildContext::new(&mut builder, &mods);

                // Feed tx: mutate creature_state → fed_creature, grounded at GSR₂.
                let feed_inputs = [(creature_state.clone(), tx_create.clone())];
                let mut feed_tx = TxBuilder::new(&mut ctx, &feed_inputs, gsr2_sr.clone());
                let tx_before_feed = feed_tx.tx.dict();
                // Pre-materialise DictContains(key) required by TxObjectStateNullified.
                let _ = ctx
                    .builder
                    .priv_op(op!(DictContains(
                        creature_state.dict(),
                        "key",
                        creature_state.key
                    )))
                    .unwrap();
                let st_tx_mutated =
                    feed_tx.mutate(&mut ctx, fed_creature.clone(), creature_state.clone());

                // Prove IsCreature(creature_state) by replaying NewCreature inline.
                // This uses a separate TxBuilder for the creation tx; its statements
                // are private sub-proofs in the same MultiPodBuilder.
                let mut create_replay = TxBuilder::new(&mut ctx, &[], gsr1_sr.clone());
                let st_new_creature = prove_new_creature_stmts(
                    &mut ctx,
                    &mut create_replay,
                    &gsr1_sr,
                    gsr1_block,
                    create_timeout,
                );
                // IsCreature is OR(NewCreature, Feed); take the NewCreature branch.
                let st_is_creature =
                    st_custom!(ctx, IsCreature() = (st_new_creature, Statement::None)).unwrap();

                // NotExpired: creature's timeout_block (400) >= grounding block (200).
                let grounding_gsr = Arc::clone(&feed_tx.tx.state_root);
                let st_not_expired = tx_utils::not_expired(
                    &mut ctx,
                    &grounding_gsr,
                    tx_before_feed,
                    &creature_state,
                );

                // Renewal: new_timeout = gsr2_block + 300 = 500.
                let st_sum_feed = ctx
                    .builder
                    .priv_op(op!(SumOf(feed_timeout, gsr2_block, 300_i64)))
                    .unwrap();
                let st_dict_update = ctx
                    .builder
                    .priv_op(op!(DictUpdate(
                        fed_creature.dict(),
                        creature_state.dict(),
                        "timeout_block",
                        feed_timeout
                    )))
                    .unwrap();

                st_custom!(
                    ctx,
                    Feed() = (
                        st_is_creature,
                        st_not_expired,
                        st_sum_feed,
                        st_dict_update,
                        st_tx_mutated
                    )
                )
                .unwrap();

                let (st_finalized, _) = feed_tx.finalize(&mut ctx);
                ctx.builder.reveal(&st_finalized).unwrap();
            }
            builder
                .solve()
                .unwrap()
                .prove(&MockProver {})
                .unwrap()
                .output_pod()
                .clone()
        };

        create_pod.pod.verify().unwrap();
        feed_pod.pod.verify().unwrap();
    }
}
