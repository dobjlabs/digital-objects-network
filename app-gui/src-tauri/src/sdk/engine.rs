use std::sync::Arc;

use anyhow::{anyhow, Result};
use common::{
    payload::{Payload, PayloadProof},
    shrink::{shrink_compress_pod, ShrunkMainPodSetup},
};
use craft_sdk::{Helper, SpendableObject, SpendableObjects};
use pod2::middleware::{Hash, Params};
use txlib::GroundingWitness;

use crate::spec;

pub(crate) fn execute_action(
    action_id: String,
    grounding_witness: GroundingWitness,
    inputs: Vec<SpendableObject>,
) -> Result<SpendableObjects> {
    let helper = Helper::new_multi_module(spec::action_groups());
    // Relayed payloads are recursively verified/compressed, which is incompatible with MockMainPod.
    let builder = helper.builder(false, Arc::new(grounding_witness));
    Ok(builder.action(&action_id, inputs))
}

pub(crate) fn build_relayer_payload(
    old_state_root_hash: &Hash,
    action_output: &SpendableObjects,
) -> Result<Vec<u8>> {
    let params = Params::default();
    let shrunk_main_pod = ShrunkMainPodSetup::new(&params)
        .build()
        .map_err(|err| anyhow!("failed to build shrunk proof circuit: {err}"))?;
    let compressed = shrink_compress_pod(&shrunk_main_pod, action_output.tx_pod.clone())
        .map_err(|err| anyhow!("failed to shrink/compress tx proof: {err}"))?;

    let tx_final = action_output.tx.dict().commitment();
    let nullifiers = action_output
        .tx
        .nullifiers
        .iter()
        .map(|entry| Ok(Hash(entry?.raw().0)))
        .collect::<Result<Vec<_>>>()?;
    let payload = Payload {
        proof: PayloadProof::Plonky2(Box::new(compressed)),
        tx_final,
        state_root_hash: *old_state_root_hash,
        nullifiers,
    };

    Ok(payload.to_bytes())
}
