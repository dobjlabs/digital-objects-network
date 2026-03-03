use std::sync::Arc;

use pod2::lang::{self, load_module};

const TXLIB_HASH_PLACEHOLDER: &str = "0xTXLIB_MODULE_HASH";

pub fn module() -> Result<lang::Module, lang::LangError> {
    log::info!("Loading time example predicates");
    let params = pod2::middleware::Params::default();
    let txlib_module = Arc::new(txlib::predicates::module());
    let txlib_hash = format!("{:#}", txlib_module.id());
    let source = include_str!("time.podlang").replace(TXLIB_HASH_PLACEHOLDER, &txlib_hash);
    load_module(&source, "txlib_examples_time", &params, &[txlib_module])
}

#[cfg(test)]
mod tests {
    use pod2::{
        backends::plonky2::mock::mainpod::MockProver,
        frontend::MultiPodBuilder,
        middleware::{Params, Statement, VDSet, Value, containers::Array},
    };
    use pod2utils::{macros::BuildContext, op, set, st_custom};

    use super::*;
    use crate::test_utils::MockStateRoot;

    /// Proves StateRoot for a mock GSR. Returns StateRoot statement.
    fn prove_state_root(ctx: &mut BuildContext, gsr: &MockStateRoot) -> Statement {
        st_custom!(
            ctx,
            StateRoot() = (
                HashOf(gsr.tx_nullifiers_hash, gsr.txs, gsr.nullifiers),
                HashOf(gsr.block_number_gsrs_hash, gsr.block_number, gsr.gsrs),
                HashOf(gsr.hash, gsr.tx_nullifiers_hash, gsr.block_number_gsrs_hash)
            )
        )
        .unwrap()
    }

    #[test]
    fn test_time_example_predicates_exist() {
        let module = module().unwrap();

        module.predicate_ref_by_name("LockObject").unwrap();
        module.predicate_ref_by_name("UnlockObject").unwrap();
        module
            .predicate_ref_by_name("BlockNumberForStateRoot")
            .unwrap();
        module
            .predicate_ref_by_name("PriorStateRootInStateRoot")
            .unwrap();
        module
            .predicate_ref_by_name("DistanceBetweenStateRoots")
            .unwrap();
        module.predicate_ref_by_name("ExpiringOption").unwrap();
        module.predicate_ref_by_name("ExecuteOption").unwrap();
    }

    /// Demonstrates time-locked objects across two independently-generated proofs.
    ///
    /// The scenario:
    /// - An object `{key: 42, power: 100}` starts unlocked.
    ///
    /// **POD 1 (lock proof):** A transaction inserts the object and then locks it with a
    /// minimum duration of 10 blocks by adding a `"locked"` field. The transaction is
    /// finalized, producing GSR₂.
    ///
    /// **Time passage:** GSR₃ is produced at a later block. Because the GSR sets are
    /// grow-only, GSR₃ carries forward all of GSR₂'s transactions and nullifiers, and
    /// its `gsrs` array includes both GSR₁ and GSR₂.
    ///
    /// **POD 2 (unlock proof):** A transaction, grounded in GSR₃, inserts the locked
    /// object and then unlocks it by proving that at least 10 blocks have elapsed since
    /// GSR₂ (when the lock was established). The transaction is finalized.
    #[test]
    fn prove_lock_and_unlock() {
        use std::collections::HashMap;
        use txlib::{Object, StateRoot as TxStateRoot, TxBuilder};

        let txlib_module = Arc::new(txlib::predicates::module());
        let time_module = Arc::new(module().unwrap());

        let duration = 10_i64;
        let gsr2_block = 5_i64;
        let gsr3_block = gsr2_block + duration + 1; // 16; distance from gsr2 is 11
        let distance = gsr3_block - gsr2_block;

        let params = Params::default();
        let vd_set = VDSet::new(&[]);

        // Object states: unlocked_obj gains a "locked" field to become locked_obj.
        let unlocked_obj = Object::new(HashMap::new());
        let mut locked_obj = unlocked_obj.clone();
        locked_obj
            .app_layer
            .insert("locked".to_string(), Value::from(duration));

        // GSR₁: block 0, empty state.
        let gsr1_sr = Arc::new(TxStateRoot {
            block_number: 0,
            transactions: set!(),
            nullifiers: set!(),
            gsrs: Array::new(vec![]),
        });

        // === POD 1: Lock transaction, grounded in GSR₁ ===
        let (lock_pod, tx_lock) = {
            let mut builder = MultiPodBuilder::new(&params, &vd_set);
            let tx_lock = {
                let txlib_mods = [Arc::clone(&txlib_module)];
                let mut ctx = BuildContext::new(&mut builder, &txlib_mods);

                let mut tx_builder = TxBuilder::new(&mut ctx, &[], gsr1_sr.clone());
                // Insert unlocked_obj then immediately mutate it to locked_obj.
                tx_builder.insert(&mut ctx, unlocked_obj.clone());
                let st_tx_mutated =
                    tx_builder.mutate(&mut ctx, locked_obj.clone(), unlocked_obj.clone());

                // Prove LockObject using the time module.
                {
                    let time_mods = [Arc::clone(&time_module)];
                    let time_ctx = BuildContext::new(&mut *ctx.builder, &time_mods);
                    st_custom!(
                        time_ctx,
                        LockObject() = (
                            DictInsert(locked_obj.dict(), unlocked_obj.dict(), "locked", duration),
                            st_tx_mutated.clone()
                        )
                    )
                    .unwrap();
                }

                let (st_finalized, tx_lock) = tx_builder.finalize(&mut ctx);
                ctx.builder.reveal(&st_finalized).unwrap();
                tx_lock
            };
            let pod = builder
                .solve()
                .unwrap()
                .prove(&MockProver {})
                .unwrap()
                .output_pod()
                .clone();
            (pod, tx_lock)
        };

        // GSR₂: block 5, contains the lock transaction.
        let mut gsr2_txs = set!();
        gsr2_txs.insert(&Value::from(tx_lock.dict())).unwrap();
        let gsr2_nullifiers = tx_lock.nullifiers.clone();
        let gsr2_gsrs = Array::new(vec![Value::from(gsr1_sr.hash())]);
        let gsr2_sr = Arc::new(TxStateRoot {
            block_number: gsr2_block,
            transactions: gsr2_txs.clone(),
            nullifiers: gsr2_nullifiers.clone(),
            gsrs: gsr2_gsrs.clone(),
        });
        let gsr2_mock = MockStateRoot::new(
            gsr2_block,
            gsr2_txs.clone(),
            gsr2_nullifiers.clone(),
            gsr2_gsrs,
        );

        // GSR₃: block 16, same transactions/nullifiers, gsrs array extended with GSR₂.
        let gsr3_gsrs = Array::new(vec![
            Value::from(gsr1_sr.hash()),
            Value::from(gsr2_sr.hash()),
        ]);
        let gsr3_sr = Arc::new(TxStateRoot {
            block_number: gsr3_block,
            transactions: gsr2_txs.clone(),
            nullifiers: gsr2_nullifiers.clone(),
            gsrs: gsr3_gsrs.clone(),
        });
        let gsr3_mock = MockStateRoot::new(gsr3_block, gsr2_txs, gsr2_nullifiers, gsr3_gsrs);

        // === POD 2: Unlock transaction, grounded in GSR₃ ===
        let unlock_pod = {
            let mut builder = MultiPodBuilder::new(&params, &vd_set);
            {
                let txlib_mods = [Arc::clone(&txlib_module)];
                let mut ctx = BuildContext::new(&mut builder, &txlib_mods);

                // locked_obj arrived via tx_lock; TxBuilder proves it is grounded in GSR₃.
                let inputs = [(locked_obj.clone(), tx_lock.clone())];
                let mut tx_builder = TxBuilder::new(&mut ctx, &inputs, gsr3_sr.clone());

                // Capture tx_before before the mutation (needed for UnlockObject).
                let tx_before = tx_builder.tx.dict();
                let st_tx_mutated =
                    tx_builder.mutate(&mut ctx, unlocked_obj.clone(), locked_obj.clone());

                // Prove state roots for GSR₂ and GSR₃.
                let st_gsr2_root = prove_state_root(&mut ctx, &gsr2_mock);
                let st_gsr3_root = prove_state_root(&mut ctx, &gsr3_mock);

                // Prove tx_lock was recorded in GSR₂ (UnlockObject clause 2).
                let st_gsr2_has_tx = ctx
                    .builder
                    .priv_op(op!(SetContains(gsr2_mock.txs, tx_lock.dict())))
                    .unwrap();
                let st_tx_in_gsr2 = st_custom!(
                    ctx,
                    TxInStateRoot() = (st_gsr2_root.clone(), st_gsr2_has_tx)
                )
                .unwrap();

                {
                    let time_mods = [Arc::clone(&time_module)];
                    let time_ctx = BuildContext::new(&mut *ctx.builder, &time_mods);

                    // DistanceBetweenStateRoots: GSR₃ contains GSR₂ in its gsrs array at index 1.
                    let st_gsr3_has_gsr2 = time_ctx
                        .builder
                        .priv_op(op!(ArrayContains(gsr3_mock.gsrs, 1_i64, gsr2_mock.hash)))
                        .unwrap();
                    let st_prior_gsr = st_custom!(
                        time_ctx,
                        PriorStateRootInStateRoot() = (st_gsr3_root.clone(), st_gsr3_has_gsr2)
                    )
                    .unwrap();
                    let st_gsr3_block_num = st_custom!(
                        time_ctx,
                        BlockNumberForStateRoot(block_number = gsr3_block) = (st_gsr3_root)
                    )
                    .unwrap();
                    let st_gsr2_block_num = st_custom!(
                        time_ctx,
                        BlockNumberForStateRoot(block_number = gsr2_block) = (st_gsr2_root)
                    )
                    .unwrap();
                    let st_distance = st_custom!(
                        time_ctx,
                        DistanceBetweenStateRoots(distance = distance) = (
                            st_prior_gsr,
                            st_gsr3_block_num,
                            st_gsr2_block_num,
                            SumOf(gsr3_block, gsr2_block, distance)
                        )
                    )
                    .unwrap();

                    // Remaining UnlockObject clause statements.
                    let st_tx_before_root = time_ctx
                        .builder
                        .priv_op(op!(DictContains(
                            tx_before,
                            "state_root_hash",
                            gsr3_sr.hash()
                        )))
                        .unwrap();
                    let st_locked_in_tx = time_ctx
                        .builder
                        .priv_op(op!(SetContains(
                            (&tx_lock.dict(), "live"),
                            locked_obj.dict()
                        )))
                        .unwrap();
                    let st_gt_eq = time_ctx
                        .builder
                        .priv_op(op!(GtEq(distance, (&locked_obj.dict(), "locked"))))
                        .unwrap();
                    let st_dict_delete = time_ctx
                        .builder
                        .priv_op(op!(DictDelete(
                            unlocked_obj.dict(),
                            locked_obj.dict(),
                            "locked"
                        )))
                        .unwrap();

                    // UnlockObject has 7 AND-clauses; use apply_predicate_with.
                    struct ApplyErr(pod2::frontend::MultiPodError);
                    impl From<pod2::lang::MultiOperationError> for ApplyErr {
                        fn from(e: pod2::lang::MultiOperationError) -> Self {
                            ApplyErr(pod2::frontend::MultiPodError::Custom(e.to_string()))
                        }
                    }
                    time_module
                        .apply_predicate_with(
                            "UnlockObject",
                            vec![
                                st_tx_before_root,
                                st_tx_in_gsr2,
                                st_locked_in_tx,
                                st_distance,
                                st_gt_eq,
                                st_dict_delete,
                                st_tx_mutated.clone(),
                            ],
                            false,
                            |is_public, op| -> Result<Statement, ApplyErr> {
                                if is_public {
                                    time_ctx.builder.pub_op(op).map_err(ApplyErr)
                                } else {
                                    time_ctx.builder.priv_op(op).map_err(ApplyErr)
                                }
                            },
                        )
                        .map_err(|ApplyErr(e)| e)
                        .unwrap();
                }

                let (st_finalized, _) = tx_builder.finalize(&mut ctx);
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

        lock_pod.pod.verify().unwrap();
        unlock_pod.pod.verify().unwrap();
    }

    /// Demonstrates expiry: an option with a `timeout_block` is exercised in a single
    /// transaction whose grounding GSR has block number ≤ `timeout_block`.
    ///
    /// The synchronizer enforces that the grounding GSR is at most one window (~300 blocks)
    /// old, so `timeout_block` is a "times out at block N" deadline with ~300-block fuzz
    /// rather than a precise wall-clock expiry.
    ///
    /// Scenario: option `{key, work, value: 42, timeout_block: 500}` is exercised in a
    /// transaction grounded at block 400, producing `{key, work, value: 42}`.
    #[test]
    fn prove_expiry_example() {
        use std::collections::HashMap;
        use txlib::{Object, StateRoot as TxStateRoot, TxBuilder};

        let txlib_module = Arc::new(txlib::predicates::module());
        let time_module = Arc::new(module().unwrap());

        let gsr_block = 400_i64;
        let timeout_block = 500_i64;
        let obj_value = 42_i64;

        let params = Params::default();
        let vd_set = VDSet::new(&[]);

        // option_obj: {key, work, value: 42, timeout_block: 500}
        let option_obj = Object::new(HashMap::from([
            ("value".to_string(), Value::from(obj_value)),
            ("timeout_block".to_string(), Value::from(timeout_block)),
        ]));
        // executed_obj: timeout_block removed
        let mut executed_obj = option_obj.clone();
        executed_obj.app_layer.remove("timeout_block");

        // Grounding GSR: block 400, empty (no prior transactions).
        let gsr_sr = Arc::new(TxStateRoot {
            block_number: gsr_block,
            transactions: set!(),
            nullifiers: set!(),
            gsrs: Array::new(vec![]),
        });
        let gsr_mock = MockStateRoot::new(gsr_block, set!(), set!(), Array::new(vec![]));

        let execute_pod = {
            let mut builder = MultiPodBuilder::new(&params, &vd_set);
            {
                let txlib_mods = [Arc::clone(&txlib_module)];
                let mut ctx = BuildContext::new(&mut builder, &txlib_mods);

                let mut tx_builder = TxBuilder::new(&mut ctx, &[], gsr_sr.clone());
                // Insert option_obj then mutate to executed_obj (removes timeout_block).
                // Materialise DictContains(option_obj, "key", ...) before mutate so that
                // the HashOf inside TxObjectStateNullified doesn't forward-reference it.
                let _ = ctx
                    .builder
                    .priv_op(op!(DictContains(option_obj.dict(), "key", option_obj.key)))
                    .unwrap();
                tx_builder.insert(&mut ctx, option_obj.clone());
                let tx_before = tx_builder.tx.dict();
                let st_tx_mutated =
                    tx_builder.mutate(&mut ctx, executed_obj.clone(), option_obj.clone());

                // Prove the grounding GSR's block number.
                let st_gsr_root = prove_state_root(&mut ctx, &gsr_mock);

                {
                    let time_mods = [Arc::clone(&time_module)];
                    let time_ctx = BuildContext::new(&mut *ctx.builder, &time_mods);

                    // Prove ExpiringOption structure.
                    let st_expiring = st_custom!(
                        time_ctx,
                        ExpiringOption(timeout_block = timeout_block) = (
                            DictContains(option_obj.dict(), "key", option_obj.key),
                            DictContains(option_obj.dict(), "value", obj_value),
                            DictContains(option_obj.dict(), "timeout_block", timeout_block)
                        )
                    )
                    .unwrap();

                    // Prove grounding GSR block number and the timeout constraint.
                    let st_state_root_hash = time_ctx
                        .builder
                        .priv_op(op!(DictContains(
                            tx_before,
                            "state_root_hash",
                            gsr_sr.hash()
                        )))
                        .unwrap();
                    let st_block_num = st_custom!(
                        time_ctx,
                        BlockNumberForStateRoot(block_number = gsr_block) = (st_gsr_root)
                    )
                    .unwrap();
                    let st_gt_eq = time_ctx
                        .builder
                        .priv_op(op!(GtEq(timeout_block, gsr_block)))
                        .unwrap();
                    let st_dict_delete = time_ctx
                        .builder
                        .priv_op(op!(DictDelete(
                            executed_obj.dict(),
                            option_obj.dict(),
                            "timeout_block"
                        )))
                        .unwrap();

                    // ExecuteOption has 6 AND-clauses; use apply_predicate_with.
                    struct ApplyErr(pod2::frontend::MultiPodError);
                    impl From<pod2::lang::MultiOperationError> for ApplyErr {
                        fn from(e: pod2::lang::MultiOperationError) -> Self {
                            ApplyErr(pod2::frontend::MultiPodError::Custom(e.to_string()))
                        }
                    }
                    time_module
                        .apply_predicate_with(
                            "ExecuteOption",
                            vec![
                                st_expiring,
                                st_state_root_hash,
                                st_block_num,
                                st_gt_eq,
                                st_dict_delete,
                                st_tx_mutated.clone(),
                            ],
                            false,
                            |is_public, op| -> Result<Statement, ApplyErr> {
                                if is_public {
                                    time_ctx.builder.pub_op(op).map_err(ApplyErr)
                                } else {
                                    time_ctx.builder.priv_op(op).map_err(ApplyErr)
                                }
                            },
                        )
                        .map_err(|ApplyErr(e)| e)
                        .unwrap();
                }

                let (st_finalized, _) = tx_builder.finalize(&mut ctx);
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

        execute_pod.pod.verify().unwrap();
    }
}
