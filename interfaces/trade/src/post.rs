//! Posting a finalized joint transaction: shrink the tx pod into the
//! blob-sized circuit, wrap it as a relayer payload, submit, and wait
//! for on-chain confirmation. Mirrors the driver's single-party
//! posting path.

use anyhow::{Result, anyhow};
use driver::{
    HttpRelayerClient, RELAYER_CONFIRM_TIMEOUT_SECS, RELAYER_POLL_INTERVAL_MS,
    RELAYER_TX_HASH_TIMEOUT_SECS, RelayerClient, RelayerConfirmation,
};
use payload::{
    payload::{Payload, PayloadProof},
    shrink::{ShrunkMainPodSetup, shrink_compress_pod},
};
use pod2::{
    frontend::MainPod,
    middleware::{Hash, Params},
};
use txlib::Tx;

pub fn build_payload_bytes(state_root: Hash, tx: &Tx, tx_pod: MainPod) -> Result<Vec<u8>> {
    let params = Params::default();
    let setup = ShrunkMainPodSetup::new(&params)
        .build()
        .map_err(|err| anyhow!("cannot build the shrink circuit: {err}"))?;
    let compressed = shrink_compress_pod(&setup, tx_pod)
        .map_err(|err| anyhow!("cannot shrink the transaction proof: {err}"))?;
    let payload = Payload {
        proof: PayloadProof::Plonky2(Box::new(compressed)),
        tx_final: tx.dict().commitment(),
        state_root,
        nullifiers: tx.nullifier_hashes()?,
        live: tx.live_commitments()?,
    };
    Ok(payload.to_bytes())
}

pub fn post_and_confirm(
    relayer_api_url: &str,
    payload_bytes: &[u8],
) -> Result<RelayerConfirmation> {
    let client = HttpRelayerClient::new();
    let submitted = client.submit_proof(relayer_api_url, payload_bytes, None)?;
    let tx_hash = client.wait_for_tx_hash(
        relayer_api_url,
        &submitted.job_id,
        RELAYER_TX_HASH_TIMEOUT_SECS,
        RELAYER_POLL_INTERVAL_MS,
    )?;
    let mut confirmation = client.wait_for_confirmation(
        relayer_api_url,
        &submitted.job_id,
        RELAYER_CONFIRM_TIMEOUT_SECS,
        RELAYER_POLL_INTERVAL_MS,
    )?;
    confirmation.tx_hash.get_or_insert(tx_hash);
    Ok(confirmation)
}
