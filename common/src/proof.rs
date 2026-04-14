use std::collections::HashSet;

use crate::{
    payload::{Payload, PayloadProof},
    shrink::cache_get_shrunk_main_pod_circuit_data,
};
use anyhow::{Result, anyhow};
use plonky2::plonk::proof::CompressedProofWithPublicInputs;
use pod2::{
    backends::plonky2::{basetypes::DEFAULT_VD_SET, mainpod::calculate_statements_hash},
    middleware::{
        CommonCircuitData, CustomPredicateRef, F, Hash, Params, Statement, Value,
        VerifierCircuitData, containers::Set,
    },
};

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

/// Mock parser: accepts JSON `{ "tx_final": "0x...", "nullifiers": [...], "state_root_hash": "0x..." }`
/// Hash bytes serialized as lowercase hex strings.
/// Returns a `Payload` with a dummy `PayloadProof::Groth16(vec![])` since no real proof is needed
/// for mock testing.
#[cfg(any(test, feature = "test-utils"))]
pub struct MockBlobParser;

#[cfg(any(test, feature = "test-utils"))]
#[derive(serde::Deserialize)]
struct MockProofJson {
    tx_final: String,
    nullifiers: Vec<String>,
    state_root_hash: String,
}

#[cfg(any(test, feature = "test-utils"))]
pub fn parse_hex_hash(s: &str) -> Result<Hash> {
    use anyhow::Context;

    let hex = s.strip_prefix("0x").unwrap_or(s);
    <Hash as hex::FromHex>::from_hex(hex).context("Invalid Hash hex")
}

#[cfg(any(test, feature = "test-utils"))]
impl BlobParser for MockBlobParser {
    fn parse_blob(&self, blob_bytes: &[u8]) -> Result<Option<Payload>> {
        let json: MockProofJson = match serde_json::from_slice(blob_bytes) {
            Ok(j) => j,
            Err(_) => return Ok(None),
        };
        let tx_final = parse_hex_hash(&json.tx_final)?;
        let nullifiers = json
            .nullifiers
            .iter()
            .map(|s| parse_hex_hash(s))
            .collect::<Result<Vec<_>>>()?;
        let state_root_hash = parse_hex_hash(&json.state_root_hash)?;
        Ok(Some(Payload {
            proof: PayloadProof::Groth16(vec![]),
            tx_final,
            state_root_hash,
            nullifiers,
        }))
    }
}

// ---------------------------------------------------------------------------
// ProofParser — real Plonky2 proof verification
// ---------------------------------------------------------------------------

/// Parses binary blob payloads (`common::payload::Payload`) and verifies the
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

        // Load Groth16 verification key so verify_groth16_proof can call groth16_verify.
        #[cfg(feature = "groth16")]
        crate::groth::load_vk()
            .map_err(|e| anyhow!("failed to load Groth16 verification key: {e}"))?;

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
    /// `[statements_hash (4 field elems) || vds_root (4 field elems)]`.
    /// We reconstruct those expected public inputs from `st` and `self.vds_root`, then
    /// decompress and verify the Plonky2 proof.
    fn verify_shrunk_main_pod(&self, proof: PayloadProof, st: Statement) -> Result<()> {
        let sts_hash = calculate_statements_hash(&[st.into()]);
        let public_inputs = [sts_hash.0, self.vds_root.0].concat();
        match proof {
            PayloadProof::Plonky2(compressed_proof) => {
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
            #[cfg(feature = "groth16")]
            PayloadProof::Groth16(framed) => {
                self.verify_groth16_proof(&framed, public_inputs)
            }
            #[cfg(not(feature = "groth16"))]
            PayloadProof::Groth16(_) => {
                Err(anyhow!("Groth16 proof received but 'groth16' feature is not enabled"))
            }
        }
    }

    #[cfg(feature = "groth16")]
    fn verify_groth16_proof(&self, framed: &[u8], public_inputs: Vec<F>) -> Result<()> {
        // Decode framed bytes: [proof_len: u32 LE] [proof] [public_inputs]
        if framed.len() < 4 {
            return Err(anyhow!("Groth16 framed payload too short"));
        }
        let proof_len =
            u32::from_le_bytes(framed[..4].try_into().unwrap()) as usize;
        if framed.len() < 4 + proof_len {
            return Err(anyhow!(
                "Groth16 framed payload truncated: expected {} proof bytes, got {}",
                proof_len,
                framed.len() - 4
            ));
        }
        let g16_proof = framed[4..4 + proof_len].to_vec();
        // Re-encode public inputs in Gnark's format for verification
        let g16_pub_inp = pod2_onchain::encode_public_inputs_gnark(public_inputs);
        pod2_onchain::groth16_verify(g16_proof, g16_pub_inp)
            .map_err(|e| anyhow!("Groth16 proof verification failed: {e}"))
    }
}

impl BlobParser for ProofParser {
    fn parse_blob(&self, blob_bytes: &[u8]) -> Result<Option<Payload>> {
        // 1. Decode payload using common::payload; return None if not our format.
        let payload = match Payload::from_bytes(blob_bytes, &self.common_circuit_data) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };

        // 2. Reconstruct the nullifiers Set and build the expected Statement::Custom.
        //    txlib's TxFinalized(tx_final, tx_nullifiers, state_root_hash):
        //      - tx_final:        Value::from(tx dict commitment) = Value::from(payload.tx_final)
        //      - tx_nullifiers:   Set reconstructed from payload.nullifiers
        //      - state_root_hash: payload.state_root_hash
        let nullifiers_set = Set::new(
            payload
                .nullifiers
                .iter()
                .map(|h| Value::from(*h))
                .collect::<HashSet<_>>(),
        );
        let statement = Statement::Custom(
            self.txn_finalized_pred.clone(),
            vec![
                Value::from(payload.tx_final),
                Value::from(nullifiers_set),
                Value::from(payload.state_root_hash),
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
            r#"{{"tx_final":"{}","nullifiers":["{}"],"state_root_hash":"{}"}}"#,
            tx_hex, null_hex, sr_hex
        );

        let result = parser.parse_blob(json.as_bytes()).unwrap();
        assert!(result.is_some());
        let payload = result.unwrap();
        assert_eq!(payload.tx_final, tx_final);
        assert_eq!(payload.nullifiers.len(), 1);
        assert_eq!(payload.nullifiers[0], nullifier);
        assert_eq!(payload.state_root_hash, state_root);
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
            r#"{{"tx_final":"{}","nullifiers":[],"state_root_hash":"{}"}}"#,
            tx_hex, tx_hex
        );

        let result = parser.parse_blob(json.as_bytes()).unwrap();
        assert!(result.is_some());
        let payload = result.unwrap();
        assert_eq!(payload.tx_final, tx_final);
    }
}
