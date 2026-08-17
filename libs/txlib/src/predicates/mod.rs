use std::sync::Arc;

use pod2::lang::{self, load_module};

const TX_EVENTS_HASH_PLACEHOLDER: &str = "0xTX_EVENTS_MODULE_HASH";

#[cfg(test)]
/// Load the test crafting predicates (simplified, no VDF).
pub fn crafting_test_module() -> lang::Module {
    let params = pod2::middleware::Params::default();
    let events = Arc::new(events_module());
    let events_hash = format!("{:#}", events.batch.id());
    let source =
        include_str!("crafting_test.podlang").replace(TX_EVENTS_HASH_PLACEHOLDER, &events_hash);
    load_module(&source, "craft", &params, &[events]).expect("crafting_test.podlang compiles")
}

#[cfg(test)]
/// Load a second, independent plugin batch. Used to prove a transaction
/// can carry top-level actions guarded by two different batches.
pub fn swap_test_module() -> lang::Module {
    let params = pod2::middleware::Params::default();
    let events = Arc::new(events_module());
    let events_hash = format!("{:#}", events.batch.id());
    let source =
        include_str!("swap_test.podlang").replace(TX_EVENTS_HASH_PLACEHOLDER, &events_hash);
    load_module(&source, "swap", &params, &[events]).expect("swap_test.podlang compiles")
}

#[cfg(test)]
/// Load a batch that imports [`swap_test_module`] and calls its action as a
/// sub-action clause, to check whether a plugin batch can depend on
/// another rather than only sit beside it in a transaction.
pub fn import_test_module() -> lang::Module {
    let params = pod2::middleware::Params::default();
    let events = Arc::new(events_module());
    let swap = Arc::new(swap_test_module());
    let source = include_str!("import_test.podlang")
        .replace(
            TX_EVENTS_HASH_PLACEHOLDER,
            &format!("{:#}", events.batch.id()),
        )
        .replace("0xSWAP_MODULE_HASH", &format!("{:#}", swap.batch.id()));
    load_module(&source, "imp", &params, &[events, swap]).expect("import_test.podlang compiles")
}

/// The chain-primitive event predicates (TxInsert/TxMutate/TxDelete).
/// Kept in their own batch so action predicates and recorded
/// transactions keep stable hashes across edits to the replay and
/// finalize predicates in [`module`].
pub fn events_module() -> lang::Module {
    let params = pod2::middleware::Params::default();
    load_module(include_str!("tx_events.podlang"), "txev", &params, &[])
        .expect("tx_events.podlang compiles")
}

/// The replay/grounding/finalize predicates. Imports [`events_module`]
/// for the chain primitives.
pub fn module() -> lang::Module {
    let params = pod2::middleware::Params::default();
    let events = Arc::new(events_module());
    let events_hash = format!("{:#}", events.batch.id());
    let source = include_str!("txlib.podlang").replace(TX_EVENTS_HASH_PLACEHOLDER, &events_hash);
    load_module(&source, "tx", &params, &[events]).expect("txlib.podlang compiles")
}

#[cfg(test)]
mod tests {

    use super::*;

    // A batch may import another batch and call its predicates. Note the
    // consequence: the call embeds the imported batch's id in this batch's
    // statement templates, so this module's id -- and every class hash in it
    // -- moves whenever the imported plugin changes.
    #[test]
    fn test_import_test_predicates_exist() {
        let module = import_test_module();
        module.predicate_ref_by_name("ClaimAndReceipt").unwrap();
        module.predicate_ref_by_name("IsReceipt").unwrap();
    }

    #[test]
    fn test_crafting_predicates_exist() {
        let module = crafting_test_module();
        // Deletion sub-actions
        module.predicate_ref_by_name("DeleteLog").unwrap();
        module.predicate_ref_by_name("DeleteWood").unwrap();
        module.predicate_ref_by_name("DeleteStick").unwrap();
        // Actions
        module.predicate_ref_by_name("FindLog").unwrap();
        module.predicate_ref_by_name("CraftWood").unwrap();
        module.predicate_ref_by_name("CraftSticks").unwrap();
        module.predicate_ref_by_name("CraftWoodPick").unwrap();
        module.predicate_ref_by_name("UseWoodPick").unwrap();
        module.predicate_ref_by_name("MineStone").unwrap();
        module.predicate_ref_by_name("SpawnWoodPick").unwrap();
        // Type guards
        module.predicate_ref_by_name("IsLog").unwrap();
        module.predicate_ref_by_name("IsWood").unwrap();
        module.predicate_ref_by_name("IsStick").unwrap();
        module.predicate_ref_by_name("IsWoodPick").unwrap();
        module.predicate_ref_by_name("IsStone").unwrap();
    }

    // Every plugin module hash and every recorded transaction bakes in
    // the events batch id. If this test fails, the change is
    // interface-breaking: every plugin manifest must be regenerated and
    // existing proofs/objects no longer verify. Only then update the
    // pinned hash.
    #[test]
    fn test_events_module_hash_pinned() {
        let module = events_module();
        assert_eq!(
            format!("{:#}", module.batch.id()),
            "0xc2b96ca2c6970e4e950d09408011691c21b6c9c24610e74aec471ea53e0ace65",
        );
    }

    #[test]
    fn test_events_predicates_exist() {
        let module = events_module();

        module.predicate_ref_by_name("TxInsert").unwrap();
        module.predicate_ref_by_name("TxMutate").unwrap();
        module.predicate_ref_by_name("TxDelete").unwrap();
    }

    #[test]
    fn test_predicates_exist() {
        let module = module();
        println!("txlib id: {:#}", module.batch.id());

        // Replay structure
        module.predicate_ref_by_name("ReplayActions").unwrap();
        module.predicate_ref_by_name("ReplayActionsStep").unwrap();
        module.predicate_ref_by_name("ReplayContents").unwrap();
        module
            .predicate_ref_by_name("ReplayContentsStepInsert")
            .unwrap();
        module
            .predicate_ref_by_name("ReplayContentsStepMutate")
            .unwrap();
        module
            .predicate_ref_by_name("ReplayContentsStepDelete")
            .unwrap();
        module
            .predicate_ref_by_name("ReplayContentsStepAction")
            .unwrap();
        module.predicate_ref_by_name("ReplayElement").unwrap();
        module.predicate_ref_by_name("ReplayAction").unwrap();
        module.predicate_ref_by_name("ReplayActionInsert").unwrap();
        module.predicate_ref_by_name("ReplayInsert").unwrap();
        module.predicate_ref_by_name("ReplayMutate").unwrap();
        module.predicate_ref_by_name("ReplayDelete").unwrap();

        // Finalization
        module.predicate_ref_by_name("InputsGrounded").unwrap();
        module
            .predicate_ref_by_name("InputsGroundedSingle")
            .unwrap();
        module.predicate_ref_by_name("InputsGroundedPair").unwrap();
        module
            .predicate_ref_by_name("InputsGroundedRecursive")
            .unwrap();
        module.predicate_ref_by_name("TxFinalBindings").unwrap();
        module.predicate_ref_by_name("TxFinalized").unwrap();
    }
}
