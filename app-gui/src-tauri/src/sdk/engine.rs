use std::sync::Arc;

use anyhow::{anyhow, Result};
use common::{
    payload::{Payload, PayloadProof},
    shrink::{shrink_compress_pod, ShrunkMainPodSetup},
};
use craft_sdk::{Helper, SpendableObject, SpendableObjects};
use pod2::middleware::{Hash, Params};
use txlib::StateRoot;

use crate::spec;

pub(super) fn execute_action(
    action_id: String,
    state_root: StateRoot,
    inputs: Vec<SpendableObject>,
) -> Result<SpendableObjects> {
    let helper = Helper::new(spec::dependencies(), spec::actions());
    // Relayed payloads are recursively verified/compressed, which is incompatible with MockMainPod.
    let builder = helper.builder(false, Arc::new(state_root));
    Ok(builder.action(&action_id, inputs))
}

pub(super) fn build_relayer_payload(
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
