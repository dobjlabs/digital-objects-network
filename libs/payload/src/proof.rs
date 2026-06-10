use std::collections::HashSet;

use crate::{
    payload::{Payload, PayloadProof},
    shrink::cache_get_shrunk_main_pod_circuit_data,
};
use anyhow::{Result, anyhow};
use plonky2::plonk::proof::CompressedProofWithPublicInputs;
use pod2::{
    backends::plonky2::{basetypes::DEFAULT_VD_SET, mainpod::public_inputs},
    middleware::{
        CommonCircuitData, CustomPredicateRef, Hash, Params, Statement, Value, VerifierCircuitData,
        containers::{Array, Set},
    },
};

fn hashes_to_set(hashes: &[Hash]) -> Set {
    Set::new(
        hashes
            .iter()
            .map(|h| Value::from(*h))
            .collect::<HashSet<_>>(),
    )
}

/// Decodes and validates a raw blob payload.
///
/// Returns `Ok(Some(payload))` when the blob contains a valid, cryptographically verified
/// `TxFinalized` proof. Returns `Ok(None)` when the bytes are not in our format (the blob
/// belongs to another application and should be silently skipped). Returns `Err` only for
/// I/O or proof verification failures that warrant logging.
pub trait BlobParser: Send + Sync {
    fn parse_blob(&self, blob_bytes: &[u8]) -> Result<Option<Payload>>;
}

// ---------------------------------------------------------------------------
// MockBlobParser
// ---------------------------------------------------------------------------

/// Mock parser: accepts JSON `{ "tx_final": "0x...", "nullifiers": [...], "live": [...], "state_root": "0x..." }`
/// Hash bytes serialized as lowercase hex strings.
/// Returns a `Payload` with a dummy empty `PayloadProof` since no real proof
/// is needed for mock testing.
#[cfg(any(test, feature = "test-utils"))]
pub struct MockBlobParser;

#[cfg(any(test, feature = "test-utils"))]
#[derive(serde::Deserialize)]
struct MockProofJson {
    tx_final: String,
    nullifiers: Vec<String>,
    live: Vec<String>,
    state_root: String,
}

#[cfg(any(test, feature = "test-utils"))]
impl BlobParser for MockBlobParser {
    fn parse_blob(&self, blob_bytes: &[u8]) -> Result<Option<Payload>> {
        use crate::decode_hash_hex;
        let json: MockProofJson = match serde_json::from_slice(blob_bytes) {
            Ok(j) => j,
            Err(_) => return Ok(None),
        };
        let tx_final = decode_hash_hex(&json.tx_final)?;
        let nullifiers = json
            .nullifiers
            .iter()
            .map(|s| decode_hash_hex(s))
            .collect::<Result<Vec<_>>>()?;
        let live = json
            .live
            .iter()
            .map(|s| decode_hash_hex(s))
            .collect::<Result<Vec<_>>>()?;
        let state_root = decode_hash_hex(&json.state_root)?;
        Ok(Some(Payload {
            proof: PayloadProof::empty_for_test(),
            tx_final,
            state_root,
            nullifiers,
            live,
        }))
    }
}

// ---------------------------------------------------------------------------
// ProofParser — real Plonky2 proof verification
// ---------------------------------------------------------------------------

/// Parses binary blob payloads (`payload::payload::Payload`) and verifies the
/// embedded shrunk Plonky2 MainPod proof against txlib's `TxFinalized` custom predicate.
pub struct ProofParser {
    txn_finalized_pred: CustomPredicateRef,
    vds_root: Hash,
    common_circuit_data: CommonCircuitData,
    verifier_circuit_data: VerifierCircuitData,
}

impl ProofParser {
    /// Build a `ProofParser`, loading the shrunk MainPod circuit data from the pod2 disk cache.
    ///
    /// On first call the wrapper circuit is built and cached under `~/.cache/pod2/`; subsequent
    /// calls return the cached entry immediately. The txlib predicate module is compiled once
    /// here so it does not need to be re-parsed on every blob.
    pub fn new() -> Result<Self> {
        let params = Params::default();
        let module = txlib::predicates::module();
        let txn_finalized_pred = module
            .predicate_ref_by_name("TxFinalized")
            .ok_or_else(|| anyhow!("TxFinalized not found in txlib module"))?;
        let vds_root = DEFAULT_VD_SET.root();
        let (common_circuit_data, verifier_circuit_data) =
            &*cache_get_shrunk_main_pod_circuit_data(&params);
        Ok(Self {
            txn_finalized_pred,
            vds_root,
            common_circuit_data: (**common_circuit_data).clone(),
            verifier_circuit_data: (**verifier_circuit_data).clone(),
        })
    }

    /// Verify a shrunk MainPod proof against a single expected `Statement`.
    ///
    /// The shrunk wrapper circuit re-exposes the original MainPod public inputs unchanged:
    /// `[statements_root (4) || vds_root (4) || is_main (1)]`. We reconstruct those expected
    /// public inputs from `st` and `self.vds_root`, then decompress and verify the Plonky2 proof.
    fn verify_shrunk_main_pod(&self, proof: PayloadProof, st: Statement) -> Result<()> {
        let sts_root = Array::new(vec![Value::from(st.hash())]).commitment();
        let public_inputs = public_inputs(sts_root, self.vds_root, true);
        let PayloadProof::Plonky2(compressed_proof) = proof;
        let proof_with_pis = CompressedProofWithPublicInputs {
            proof: *compressed_proof,
            public_inputs,
        };
        let proof = proof_with_pis
            .decompress(
                &self.verifier_circuit_data.verifier_only.circuit_digest,
                &self.common_circuit_data,
            )
            .map_err(|e| anyhow!("decompress proof: {e}"))?;
        self.verifier_circuit_data
            .verify(proof)
            .map_err(|e| anyhow!("proof verification failed: {e}"))
    }
}

impl BlobParser for ProofParser {
    fn parse_blob(&self, blob_bytes: &[u8]) -> Result<Option<Payload>> {
        // 1. Decode payload using payload::payload; return None if not our format.
        let payload = match Payload::from_bytes(blob_bytes, &self.common_circuit_data) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };

        // 2. Rebuild the public statement the proof was made against. The
        //    nullifier and live sets travel as plain hash lists, so the set
        //    commitments have to be reconstructed before they can be matched.
        let statement = Statement::Custom(
            self.txn_finalized_pred.clone(),
            vec![
                Value::from(payload.state_root).into(),
                Value::from(payload.tx_final).into(),
                Value::from(hashes_to_set(&payload.nullifiers)).into(),
                Value::from(hashes_to_set(&payload.live)).into(),
            ],
        );

        // 3. Verify the proof against the statement.
        self.verify_shrunk_main_pod(payload.proof.clone(), statement)?;

        Ok(Some(payload))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use hex::ToHex;
    use pod2::middleware::EMPTY_HASH;

    fn hash_to_hex(hash: &Hash) -> String {
        hash.encode_hex()
    }

    #[test]
    fn test_mock_proof_parser_round_trip() {
        let parser = MockBlobParser;

        let tx_final = EMPTY_HASH;
        let nullifier = EMPTY_HASH;
        let state_root = EMPTY_HASH;

        let tx_hex = format!("0x{}", hash_to_hex(&tx_final));
        let null_hex = format!("0x{}", hash_to_hex(&nullifier));
        let sr_hex = format!("0x{}", hash_to_hex(&state_root));

        let json = format!(
            r#"{{"tx_final":"{}","nullifiers":["{}"],"live":[],"state_root":"{}"}}"#,
            tx_hex, null_hex, sr_hex
        );

        let result = parser.parse_blob(json.as_bytes()).unwrap();
        assert!(result.is_some());
        let payload = result.unwrap();
        assert_eq!(payload.tx_final, tx_final);
        assert_eq!(payload.nullifiers.len(), 1);
        assert_eq!(payload.nullifiers[0], nullifier);
        assert_eq!(payload.state_root, state_root);
    }

    #[test]
    fn test_mock_proof_parser_invalid_returns_none() {
        let parser = MockBlobParser;
        let result = parser.parse_blob(b"not json").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_mock_proof_parser_no_prefix() {
        let parser = MockBlobParser;

        let tx_final = EMPTY_HASH;
        let tx_hex = hash_to_hex(&tx_final); // no "0x" prefix

        let json = format!(
            r#"{{"tx_final":"{}","nullifiers":[],"live":[],"state_root":"{}"}}"#,
            tx_hex, tx_hex
        );

        let result = parser.parse_blob(json.as_bytes()).unwrap();
        assert!(result.is_some());
        let payload = result.unwrap();
        assert_eq!(payload.tx_final, tx_final);
    }
}
