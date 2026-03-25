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
    use std::sync::Arc;

    use common::test_state::TestState;
    use pod2::{
        backends::plonky2::mock::mainpod::MockProver,
        frontend::MultiPodBuilder,
        middleware::{Hash, Key, Params, VDSet},
    };
    use pod2utils::{macros::BuildContext, op};
    use txlib::{GroundingWitness, StateRoot as TxStateRoot, Tx, TxBuilder, new_obj};

    use super::*;
    use crate::tx_utils::{self, UnlockRequest, UnlockWitness};

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

    fn state_root(state: &TestState) -> TxStateRoot {
        let (transactions_root, nullifiers_root, gsrs_root) = state.roots();
        TxStateRoot::new(
            state.block_number,
            transactions_root,
            nullifiers_root,
            gsrs_root,
        )
    }

    fn grounding_witness(state: &TestState, inputs: &[Tx]) -> Arc<GroundingWitness> {
        state.build_grounding_witness(
            inputs,
            tx_hash,
            |block_number, transactions_root, nullifiers_root, gsrs_root, source_tx_proofs| {
                Arc::new(GroundingWitness::new(
                    TxStateRoot::new(block_number, transactions_root, nullifiers_root, gsrs_root),
                    source_tx_proofs,
                ))
            },
        )
    }

    fn unlock_witness(
        grounding_state: &TestState,
        prior_state: &TestState,
        prior_state_root: &TxStateRoot,
        tx_when_locked: &Tx,
    ) -> UnlockWitness {
        let tx_membership_proof = prior_state.tx_membership_proof(tx_hash(tx_when_locked));
        let (prior_state_root_index, prior_state_root_proof) =
            grounding_state.prior_state_root_membership(prior_state_root.hash());
        UnlockWitness {
            tx_membership_proof,
            prior_state_root_index,
            prior_state_root_proof,
        }
    }

    fn prematerialize_object_key(
        ctx: &mut BuildContext,
        obj: &pod2::middleware::containers::Dictionary,
    ) {
        let key = obj.get(&Key::from("key")).unwrap().unwrap();
        let _ = ctx
            .builder
            .priv_op(op!(DictContains(obj.clone(), "key", key)))
            .unwrap();
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
        module.predicate_ref_by_name("NotExpired").unwrap();
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
    #[test]
    fn prove_lock_and_unlock() {
        let txlib_module = Arc::new(txlib::predicates::module());
        let time_module = Arc::new(module().unwrap());

        let duration = 10_i64;
        let gsr2_block = 5_i64;
        let gsr3_block = gsr2_block + duration + 1; // 16; distance from gsr2 is 11

        let params = Params::default();
        let vd_set = VDSet::new(&[]);

        let unlocked_obj = new_obj();

        // GSR₁: block 0, empty state.
        let gsr1 = TestState::empty(0);
        let gsr1_root = state_root(&gsr1);

        // === POD 1: Lock transaction, grounded in GSR₁ ===
        let (lock_pod, tx_lock, locked_obj) = {
            let builder = MultiPodBuilder::new(&params, &vd_set);
            let mods = vec![Arc::clone(&txlib_module), Arc::clone(&time_module)];
            let mut ctx = BuildContext::new(builder, mods);
            let (tx_lock, locked_obj) = {
                let mut tx_builder = TxBuilder::new(&mut ctx, &[], grounding_witness(&gsr1, &[]));
                tx_builder.insert(&mut ctx, unlocked_obj.clone());
                let locked_obj =
                    tx_utils::lock_object(&mut ctx, unlocked_obj.clone(), duration).unwrap();
                prematerialize_object_key(&mut ctx, &unlocked_obj);
                tx_builder.mutate(&mut ctx, locked_obj.clone(), unlocked_obj.clone());
                let (st_finalized, tx_lock) = tx_builder.finalize(&mut ctx);
                ctx.builder.reveal(&st_finalized).unwrap();
                (tx_lock, locked_obj)
            };
            let pod = ctx
                .builder
                .solve()
                .unwrap()
                .prove(&MockProver {})
                .unwrap()
                .output_pod()
                .clone();
            (pod, tx_lock, locked_obj)
        };

        // GSR₂: block 5, contains the lock transaction.
        let gsr2 = TestState::from_txs(
            gsr2_block,
            std::slice::from_ref(&tx_lock),
            &[gsr1_root.hash()],
            tx_hash,
            tx_nullifiers,
        );
        let gsr2_root = state_root(&gsr2);

        // GSR₃: block 16, same transactions/nullifiers, gsrs array extended with GSR₂.
        let gsr3 = TestState::from_txs(
            gsr3_block,
            std::slice::from_ref(&tx_lock),
            &[gsr1_root.hash(), gsr2_root.hash()],
            tx_hash,
            tx_nullifiers,
        );
        let gsr3_root = state_root(&gsr3);

        // === POD 2: Unlock transaction, grounded in GSR₃ ===
        let unlock_pod = {
            let builder = MultiPodBuilder::new(&params, &vd_set);
            let mods = vec![Arc::clone(&txlib_module), Arc::clone(&time_module)];
            let mut ctx = BuildContext::new(builder, mods);
            {
                let inputs = [(locked_obj.clone(), tx_lock.clone())];
                let mut tx_builder = TxBuilder::new(
                    &mut ctx,
                    &inputs,
                    grounding_witness(&gsr3, std::slice::from_ref(&tx_lock)),
                );
                let tx_before_unlock = tx_builder.tx.dict();
                let unlock_witness = unlock_witness(&gsr3, &gsr2, &gsr2_root, &tx_lock);
                let unlocked = tx_utils::unlock_object(
                    &mut ctx,
                    UnlockRequest {
                        time_module: &time_module,
                        grounding_gsr: &gsr3_root,
                        tx_before: tx_before_unlock,
                        locked_obj: locked_obj.clone(),
                        tx_when_locked: &tx_lock,
                        gsr_when_locked: &gsr2_root,
                        witness: &unlock_witness,
                    },
                )
                .unwrap();
                prematerialize_object_key(&mut ctx, &locked_obj);
                tx_builder.mutate(&mut ctx, unlocked, locked_obj.clone());
                let (st_finalized, _) = tx_builder.finalize(&mut ctx);
                ctx.builder.reveal(&st_finalized).unwrap();
            }
            ctx.builder
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

    /// Demonstrates `SetExpiry` and `NotExpired` without any application-layer framing.
    ///
    /// The scenario:
    /// - A plain object starts with no `timeout_block`.
    ///
    /// **POD 1 (set expiry):** A transaction inserts the object and then uses `SetExpiry`
    /// to mutate it, adding `timeout_block = 400` (= block 100 + 300).
    ///
    /// **POD 2 (check not-expired):** A transaction grounded at block 200 (well before 400)
    /// takes the expiry-bearing object as an input and proves `NotExpired`.
    #[test]
    fn prove_set_expiry_and_not_expired() {
        let txlib_module = Arc::new(txlib::predicates::module());
        let time_module = Arc::new(module().unwrap());

        let gsr1_block = 100_i64;
        let gsr2_block = 200_i64;
        let timeout_block = gsr1_block + 300; // 400, well after gsr2_block

        let params = Params::default();
        let vd_set = VDSet::new(&[]);

        let obj = new_obj();

        // GSR₁: block 100, empty.
        let gsr1 = TestState::empty(gsr1_block);
        let gsr1_root = state_root(&gsr1);

        // === POD 1: Insert object and set its expiry ===
        let (set_pod, tx_set, obj_with_expiry) = {
            let builder = MultiPodBuilder::new(&params, &vd_set);
            let mods = vec![Arc::clone(&txlib_module), Arc::clone(&time_module)];
            let mut ctx = BuildContext::new(builder, mods);
            let (tx_set, obj_with_expiry) = {
                let mut tx_builder = TxBuilder::new(&mut ctx, &[], grounding_witness(&gsr1, &[]));
                tx_builder.insert(&mut ctx, obj.clone());
                let tx_before_set = tx_builder.tx.dict();
                let obj_with_expiry = tx_utils::set_expiry(
                    &mut ctx,
                    &gsr1_root,
                    tx_before_set,
                    obj.clone(),
                    timeout_block,
                )
                .unwrap();
                // Pre-materialise key for TxObjectStateNullified inside mutate.
                prematerialize_object_key(&mut ctx, &obj);
                tx_builder.mutate(&mut ctx, obj_with_expiry.clone(), obj.clone());
                let (st_finalized, tx_set) = tx_builder.finalize(&mut ctx);
                ctx.builder.reveal(&st_finalized).unwrap();
                (tx_set, obj_with_expiry)
            };
            let pod = ctx
                .builder
                .solve()
                .unwrap()
                .prove(&MockProver {})
                .unwrap()
                .output_pod()
                .clone();
            (pod, tx_set, obj_with_expiry)
        };

        // GSR₂: block 200, carries tx_set.
        let gsr2 = TestState::from_txs(
            gsr2_block,
            std::slice::from_ref(&tx_set),
            &[gsr1_root.hash()],
            tx_hash,
            tx_nullifiers,
        );
        let gsr2_root = state_root(&gsr2);

        // === POD 2: Prove the object is not expired at block 200 ≤ 400 ===
        let check_pod = {
            let builder = MultiPodBuilder::new(&params, &vd_set);
            let mods = vec![Arc::clone(&txlib_module), Arc::clone(&time_module)];
            let mut ctx = BuildContext::new(builder, mods);
            {
                let inputs = [(obj_with_expiry.clone(), tx_set.clone())];
                let tx_builder = TxBuilder::new(
                    &mut ctx,
                    &inputs,
                    grounding_witness(&gsr2, std::slice::from_ref(&tx_set)),
                );
                let tx_before = tx_builder.tx.dict();
                let _ = tx_utils::not_expired(&mut ctx, &gsr2_root, tx_before, obj_with_expiry)
                    .unwrap();
                let (st_finalized, _) = tx_builder.finalize(&mut ctx);
                ctx.builder.reveal(&st_finalized).unwrap();
            }
            ctx.builder
                .solve()
                .unwrap()
                .prove(&MockProver {})
                .unwrap()
                .output_pod()
                .clone()
        };

        set_pod.pod.verify().unwrap();
        check_pod.pod.verify().unwrap();
    }
}
