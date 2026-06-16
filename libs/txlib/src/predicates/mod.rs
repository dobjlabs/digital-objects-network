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
            "0x31caeb6211bb4c73c47de52d11ba49bd4f225b585a9381145393b797775501c0",
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
