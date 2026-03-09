use std::collections::HashSet;

use anyhow::{Result, anyhow};
use plonky2::plonk::proof::CompressedProofWithPublicInputs;
use pod2::{
    backends::plonky2::{basetypes::DEFAULT_VD_SET, mainpod::calculate_statements_hash},
    middleware::{
        CommonCircuitData, CustomPredicateRef, Hash, Params, Statement, Value, VerifierCircuitData,
        containers::Set,
    },
};

use crate::{
    payload::{Payload, PayloadProof},
    shrink::cache_get_shrunk_main_pod_circuit_data,
};

/// Decodes and validates a raw payload blob.
///
/// Returns:
/// - `Ok(Some(payload))` for valid proof payloads.
/// - `Ok(None)` when bytes are not in this app's payload format.
/// - `Err(_)` for decoding or verification failures.
pub trait BlobParser: Send + Sync {
    fn parse_blob(&self, blob_bytes: &[u8]) -> Result<Option<Payload>>;
}

/// Parses binary payloads (`common::payload::Payload`) and verifies the
/// embedded shrunk Plonky2 MainPod proof against txlib's `TxFinalized` predicate.
pub struct ProofParser {
    txn_finalized_pred: CustomPredicateRef,
    vds_root: Hash,
    common_circuit_data: CommonCircuitData,
    verifier_circuit_data: VerifierCircuitData,
}

impl ProofParser {
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

    fn verify_shrunk_main_pod(&self, proof: PayloadProof, st: Statement) -> Result<()> {
        let sts_hash = calculate_statements_hash(&[st.into()]);
        let public_inputs = [sts_hash.0, self.vds_root.0].concat();
        let compressed_proof = match proof {
            PayloadProof::Plonky2(proof) => proof,
            PayloadProof::Groth16(_) => {
                return Err(anyhow!("Groth16 proof verification is not yet implemented"));
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
        let payload = match Payload::from_bytes(blob_bytes, &self.common_circuit_data) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };

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

        self.verify_shrunk_main_pod(payload.proof.clone(), statement)?;
        Ok(Some(payload))
    }
}
