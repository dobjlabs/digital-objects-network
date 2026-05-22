#[cfg(test)]
use std::sync::Arc;

use pod2::lang::{self, load_module};

#[cfg(test)]
const TXLIB_HASH_PLACEHOLDER: &str = "0xTXLIB_MODULE_HASH";

#[cfg(test)]
/// Load the test crafting predicates (simplified, no VDF).
pub fn crafting_test_module() -> lang::Module {
    let params = pod2::middleware::Params::default();
    let txlib = Arc::new(module());
    let txlib_hash = format!("{:#}", txlib.batch.id());
    let source = include_str!("crafting_test.podlang").replace(TXLIB_HASH_PLACEHOLDER, &txlib_hash);
    load_module(&source, "craft", &params, &[txlib]).expect("crafting_test.podlang compiles")
}

pub fn module() -> lang::Module {
    let params = pod2::middleware::Params::default();
    load_module(include_str!("txlib.podlang"), "tx", &params, &[]).expect("txlib.podlang compiles")
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

    #[test]
    fn test_predicates_exist() {
        let module = module();
        println!("txlib id: {:#}", module.batch.id());

        // Chain primitives
        module.predicate_ref_by_name("TxInsert").unwrap();
        module.predicate_ref_by_name("TxMutate").unwrap();
        module.predicate_ref_by_name("TxDelete").unwrap();

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
        module
            .predicate_ref_by_name("InputsGroundedSingleVar")
            .unwrap();
        module.predicate_ref_by_name("InputsGroundedPair").unwrap();
        module
            .predicate_ref_by_name("InputsGroundedPairVar")
            .unwrap();
        module
            .predicate_ref_by_name("InputsGroundedTriple")
            .unwrap();
        module
            .predicate_ref_by_name("InputsGroundedRecursive")
            .unwrap();
        module.predicate_ref_by_name("TxFinalized").unwrap();
    }
}
