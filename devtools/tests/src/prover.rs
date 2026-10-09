use std::path::PathBuf;

use driver::{DriverActionCatalog, PexeCatalog};
use payload::{
    payload::{Payload, PayloadProof},
    shrink::{ShrunkMainPodBuild, ShrunkMainPodSetup, shrink_compress_pod},
};
use pod2::middleware::{Hash, Params};
use sdk::SpendableObjects;
use txlib::GroundingWitness;
use wire_types::QualifiedName;

const PLUGIN_NAME: &str = "throughput";
const PLUGIN_MANIFEST: &str = include_str!("../plugins/throughput/manifest.toml");
const PLUGIN_SCRIPT: &str = include_str!("../plugins/throughput/plugin.rhai");

pub fn find_log_action() -> QualifiedName {
    QualifiedName::new(PLUGIN_NAME, "FindLog")
}

/// A proved transaction, encoded for the relayer, with the hashes that show it landed.
pub struct PreparedTx {
    pub payload: Vec<u8>,
    pub created: Vec<Hash>,
    pub nullifiers: Vec<Hash>,
}

/// Proves actions with real proofs and shrinks them into relayer payloads.
pub struct Prover {
    catalog: PexeCatalog,
    shrink_build: ShrunkMainPodBuild,
}

impl Prover {
    /// Builds the shrink circuit, which takes a while, so build one prover per run.
    pub fn new() -> Self {
        Self {
            catalog: load_catalog(false),
            shrink_build: ShrunkMainPodSetup::new(&Params::default()).build().unwrap(),
        }
    }

    pub fn prove(&self, action: &QualifiedName, witness: GroundingWitness) -> PreparedTx {
        let state_root = witness.state_header.hash();
        let outputs = self
            .catalog
            .execute_action(action.clone(), witness, Vec::new())
            .unwrap();
        prepare_tx(&self.shrink_build, state_root, &outputs)
    }
}

impl Default for Prover {
    fn default() -> Self {
        Self::new()
    }
}

/// Pack the plugin with the module hash its script compiles to.
fn load_catalog(mock_proofs: bool) -> PexeCatalog {
    let manifest: sdk::manifest::Manifest = toml::from_str(PLUGIN_MANIFEST).unwrap();
    let module_hash = pexe::compile_module_hash(&manifest, PLUGIN_SCRIPT).unwrap();
    let manifest_toml = pexe::set_manifest_hash(PLUGIN_MANIFEST, &module_hash).unwrap();
    let bytes = pexe::pack(&manifest_toml, PLUGIN_SCRIPT).unwrap();
    PexeCatalog::from_bytes(
        [(PathBuf::from(format!("{PLUGIN_NAME}.pexe")), bytes)],
        mock_proofs,
    )
    .unwrap()
}

fn prepare_tx(
    shrink_build: &ShrunkMainPodBuild,
    state_root: Hash,
    outputs: &SpendableObjects,
) -> PreparedTx {
    let compressed = shrink_compress_pod(shrink_build, outputs.tx_pod.clone()).unwrap();
    let nullifiers = outputs.tx.nullifier_hashes().unwrap();
    let payload = Payload {
        proof: PayloadProof::Plonky2(Box::new(compressed)),
        tx_final: outputs.tx.dict().commitment(),
        state_root,
        nullifiers: nullifiers.clone(),
        live: outputs.tx.live_commitments().unwrap(),
    };
    PreparedTx {
        payload: payload.to_bytes(),
        created: outputs
            .objs
            .iter()
            .map(|spendable| spendable.obj.commitment())
            .collect(),
        nullifiers,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use pod2::middleware::EMPTY_HASH;
    use txlib::StateHeader;

    use super::*;

    fn dummy_grounding_witness() -> GroundingWitness {
        GroundingWitness::new(
            StateHeader::new(1, 1, EMPTY_HASH, EMPTY_HASH, EMPTY_HASH, EMPTY_HASH),
            HashMap::new(),
        )
    }

    /// Every FindLog must create a distinct object, or later transactions would collide with
    /// earlier ones in the created set.
    #[test]
    fn find_log_creates_a_distinct_object_each_run() {
        let catalog = load_catalog(true);
        let run = || {
            let outputs = catalog
                .execute_action(find_log_action(), dummy_grounding_witness(), Vec::new())
                .unwrap();
            assert_eq!(outputs.objs.len(), 1);
            assert!(outputs.tx.nullifier_hashes().unwrap().is_empty());
            outputs.objs[0].obj.commitment()
        };
        assert_ne!(run(), run());
    }
}
