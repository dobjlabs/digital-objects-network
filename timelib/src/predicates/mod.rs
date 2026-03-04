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
        middleware::{Params, VDSet, Value, containers::Array},
    };
    use pod2utils::{macros::BuildContext, set};

    use super::*;
    use crate::tx_utils;

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

        let params = Params::default();
        let vd_set = VDSet::new(&[]);

        let unlocked_obj = Object::new(HashMap::new());

        // GSR₁: block 0, empty state.
        let gsr1_sr = Arc::new(TxStateRoot {
            block_number: 0,
            transactions: set!(),
            nullifiers: set!(),
            gsrs: Array::new(vec![]),
        });

        // === POD 1: Lock transaction, grounded in GSR₁ ===
        let (lock_pod, tx_lock, locked_obj) = {
            let mut builder = MultiPodBuilder::new(&params, &vd_set);
            let (tx_lock, locked_obj) = {
                let mods = [Arc::clone(&txlib_module), Arc::clone(&time_module)];
                let mut ctx = BuildContext::new(&mut builder, &mods);
                let mut tx_builder = TxBuilder::new(&mut ctx, &[], gsr1_sr.clone());
                tx_builder.insert(&mut ctx, unlocked_obj.clone());
                let locked_obj = tx_utils::lock_object(
                    &mut ctx,
                    &mut tx_builder,
                    unlocked_obj.clone(),
                    duration,
                );
                let (st_finalized, tx_lock) = tx_builder.finalize(&mut ctx);
                ctx.builder.reveal(&st_finalized).unwrap();
                (tx_lock, locked_obj)
            };
            let pod = builder
                .solve()
                .unwrap()
                .prove(&MockProver {})
                .unwrap()
                .output_pod()
                .clone();
            (pod, tx_lock, locked_obj)
        };

        // GSR₂: block 5, contains the lock transaction.
        let mut gsr2_txs = set!();
        gsr2_txs.insert(&Value::from(tx_lock.dict())).unwrap();
        let gsr2_nullifiers = tx_lock.nullifiers.clone();
        let gsr2_sr = Arc::new(TxStateRoot {
            block_number: gsr2_block,
            transactions: gsr2_txs.clone(),
            nullifiers: gsr2_nullifiers.clone(),
            gsrs: Array::new(vec![Value::from(gsr1_sr.hash())]),
        });

        // GSR₃: block 16, same transactions/nullifiers, gsrs array extended with GSR₂.
        let gsr3_sr = Arc::new(TxStateRoot {
            block_number: gsr3_block,
            transactions: gsr2_txs,
            nullifiers: gsr2_nullifiers,
            gsrs: Array::new(vec![
                Value::from(gsr1_sr.hash()),
                Value::from(gsr2_sr.hash()),
            ]),
        });

        // === POD 2: Unlock transaction, grounded in GSR₃ ===
        let unlock_pod = {
            let mut builder = MultiPodBuilder::new(&params, &vd_set);
            {
                let mods = [Arc::clone(&txlib_module), Arc::clone(&time_module)];
                let mut ctx = BuildContext::new(&mut builder, &mods);
                let inputs = [(locked_obj.clone(), tx_lock.clone())];
                let mut tx_builder = TxBuilder::new(&mut ctx, &inputs, gsr3_sr.clone());
                tx_utils::unlock_object(
                    &mut ctx,
                    &time_module,
                    &mut tx_builder,
                    locked_obj.clone(),
                    &tx_lock,
                    &gsr2_sr,
                );
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

}
