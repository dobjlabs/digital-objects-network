use std::collections::HashSet;

use anyhow::{anyhow, Context, Result};
use common::{
    payload::{Payload, PayloadProof},
    shrink::cache_get_shrunk_main_pod_circuit_data,
};
use plonky2::plonk::proof::CompressedProofWithPublicInputs;
use pod2::{
    backends::plonky2::{basetypes::DEFAULT_VD_SET, mainpod::calculate_statements_hash},
    middleware::{
        containers::Set, CommonCircuitData, CustomPredicateRef, Hash, Params, Statement, Value,
        VerifierCircuitData,
    },
};

/// A minimal compilable TxnFinalized predicate.
/// The body is a placeholder (Equal(1,1)) so the podlang parser accepts it.
/// The real constraint is that the prover commits to (tx_hash, nullifiers, state_root) via a
/// Statement::Custom in their MainPod.
pub const TXN_FINALIZED_PREDICATE: &str = "
TxnFinalized(tx_final, input_nullifiers, state_root) = AND(
    Equal(1, 1)
)
";

/// Decodes and validates a raw blob payload.
///
/// Returns `Ok(Some(payload))` when the blob contains a valid, cryptographically verified
/// `TxnFinalized` proof. Returns `Ok(None)` when the bytes are not in our format (the blob
/// belongs to another application and should be silently skipped). Returns `Err` only for
/// I/O or proof verification failures that warrant logging.
pub trait BlobParser: Send + Sync {
    fn parse_blob(&self, blob_bytes: &[u8]) -> Result<Option<Payload>>;
}

// ---------------------------------------------------------------------------
// MockBlobParser
// ---------------------------------------------------------------------------

/// Mock parser: accepts JSON `{ "tx_hash": "0x...", "nullifiers": [...], "state_root_hash": "0x..." }`
/// Hash bytes serialized as lowercase hex strings.
/// Returns a `Payload` with a dummy `PayloadProof::Groth16(vec![])` since no real proof is needed
/// for mock testing.
#[cfg(test)]
pub struct MockBlobParser;

#[cfg(test)]
#[derive(serde::Deserialize)]
struct MockProofJson {
    tx_hash: String,
    nullifiers: Vec<String>,
    state_root_hash: String,
}

#[cfg(test)]
pub fn parse_hex_hash(s: &str) -> Result<Hash> {
    let hex = s.strip_prefix("0x").unwrap_or(s);
    <Hash as hex::FromHex>::from_hex(hex).context("Invalid Hash hex")
}

#[cfg(test)]
impl BlobParser for MockBlobParser {
    fn parse_blob(&self, blob_bytes: &[u8]) -> Result<Option<Payload>> {
        let json: MockProofJson = match serde_json::from_slice(blob_bytes) {
            Ok(j) => j,
            Err(_) => return Ok(None),
        };
        let tx_hash = parse_hex_hash(&json.tx_hash)?;
        let nullifiers = json
            .nullifiers
            .iter()
            .map(|s| parse_hex_hash(s))
            .collect::<Result<Vec<_>>>()?;
        let state_root_hash = parse_hex_hash(&json.state_root_hash)?;
        Ok(Some(Payload {
            proof: PayloadProof::Groth16(vec![]),
            tx_hash,
            state_root_hash,
            nullifiers,
        }))
    }
}

// ---------------------------------------------------------------------------
// ProofParser — real Plonky2 proof verification
// ---------------------------------------------------------------------------

/// Parses binary blob payloads (`common::payload::Payload`) and verifies the
/// embedded shrunk Plonky2 MainPod proof against the TxnFinalized custom predicate.
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
    /// calls return the cached entry immediately. The `TXN_FINALIZED_PREDICATE` is compiled once
    /// here so it does not need to be re-parsed on every blob.
    pub fn new() -> Result<Self> {
        let params = Params::default();
        // TODO: when txlib is integrated, use the txlib predicate instead of the hardcoded one.
        let module =
            pod2::lang::load_module(TXN_FINALIZED_PREDICATE, "txn_finalized", &params, &[])
                .context("parse TxnFinalized predicate")?;
        let txn_finalized_pred = module
            .predicate_ref_by_name("TxnFinalized")
            .ok_or_else(|| anyhow!("TxnFinalized not found in parsed batch"))?;
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
    /// `[statements_hash (4 field elems) || vds_root (4 field elems)]`.
    /// We reconstruct those expected public inputs from `st` and `self.vds_root`, then
    /// decompress and verify the Plonky2 proof.
    fn verify_shrunk_main_pod(&self, proof: PayloadProof, st: Statement) -> Result<()> {
        let sts_hash = calculate_statements_hash(&[st.into()]);
        let public_inputs = [sts_hash.0, self.vds_root.0].concat();
        let compressed_proof = match proof {
            PayloadProof::Plonky2(proof) => proof,
            PayloadProof::Groth16(_) => {
                return Err(anyhow!("Groth16 proof verification is not yet implemented"))
            }
        };
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
        // 1. Decode payload using common::payload; return None if not our format.
        let payload = match Payload::from_bytes(blob_bytes, &self.common_circuit_data) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };

        // 2. Build nullifiers Set and Statement::Custom for the TxnFinalized predicate.
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
                Value::from(payload.tx_hash),
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

        let tx_hash = EMPTY_HASH;
        let nullifier = EMPTY_HASH;
        let state_root = EMPTY_HASH;

        let tx_hex = format!("0x{}", hash_to_hex(&tx_hash));
        let null_hex = format!("0x{}", hash_to_hex(&nullifier));
        let sr_hex = format!("0x{}", hash_to_hex(&state_root));

        let json = format!(
            r#"{{"tx_hash":"{}","nullifiers":["{}"],"state_root_hash":"{}"}}"#,
            tx_hex, null_hex, sr_hex
        );

        let result = parser.parse_blob(json.as_bytes()).unwrap();
        assert!(result.is_some());
        let payload = result.unwrap();
        assert_eq!(payload.tx_hash, tx_hash);
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

        let tx_hash = EMPTY_HASH;
        let tx_hex = hash_to_hex(&tx_hash); // no "0x" prefix

        let json = format!(
            r#"{{"tx_hash":"{}","nullifiers":[],"state_root_hash":"{}"}}"#,
            tx_hex, tx_hex
        );

        let result = parser.parse_blob(json.as_bytes()).unwrap();
        assert!(result.is_some());
        let payload = result.unwrap();
        assert_eq!(payload.tx_hash, tx_hash);
    }
}
