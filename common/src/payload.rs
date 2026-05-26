use std::io::{Read, Write};

use anyhow::{Result, anyhow};
use plonky2::{
    field::types::{Field, Field64, PrimeField64},
    plonk::proof::CompressedProof,
    util::serialization::Buffer,
};
use pod2::middleware::{C, CommonCircuitData, D, F, Hash};

use crate::ProofType;

pub fn write_elems<const N: usize>(bytes: &mut Vec<u8>, elems: &[F; N]) {
    for elem in elems {
        bytes
            .write_all(&elem.to_canonical_u64().to_le_bytes())
            .expect("vec write");
    }
}

pub fn read_elems<const N: usize>(bytes: &mut impl Read) -> Result<[F; N]> {
    let mut elems = [F::ZERO; N];
    let mut elem_bytes = [0; 8];
    #[allow(clippy::needless_range_loop)]
    for i in 0..N {
        bytes.read_exact(&mut elem_bytes)?;
        let n = u64::from_le_bytes(elem_bytes);
        if n >= F::ORDER {
            return Err(anyhow!("{n} >= F::ORDER"));
        }
        elems[i] = F::from_canonical_u64(n);
    }
    Ok(elems)
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub struct Payload {
    pub proof: PayloadProof,
    /// Commitment of the finalized transaction dictionary `{live, nullifiers, chain_start, chain_end}`.
    pub tx_final: Hash,
    pub state_root_hash: Hash,
    pub nullifiers: Vec<Hash>,
}

const PAYLOAD_MAGIC: u16 = 0xd10b;

impl Payload {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buffer = Vec::new();
        buffer
            .write_all(&PAYLOAD_MAGIC.to_le_bytes())
            .expect("vec write");
        self.proof.write_bytes(&mut buffer);
        write_elems(&mut buffer, &self.tx_final.0);
        write_elems(&mut buffer, &self.state_root_hash.0);
        assert!(self.nullifiers.len() <= 255);
        buffer
            .write_all(&(self.nullifiers.len() as u8).to_le_bytes())
            .expect("vec write");
        for nullifier in &self.nullifiers {
            write_elems(&mut buffer, &nullifier.0);
        }
        buffer
    }

    pub fn from_bytes(bytes: &[u8], common_data: &CommonCircuitData) -> Result<Self> {
        let mut bytes = bytes;
        let magic = {
            let mut buffer = [0; 2];
            bytes.read_exact(&mut buffer)?;
            u16::from_le_bytes(buffer)
        };
        if magic != PAYLOAD_MAGIC {
            return Err(anyhow!("Invalid payload magic: {magic:04x}"));
        }

        let (proof, len) = PayloadProof::from_bytes(bytes, common_data)?;
        bytes = &bytes[len..];
        let tx_final = Hash(read_elems(&mut bytes)?);
        let state_root_hash = Hash(read_elems(&mut bytes)?);
        let nullifiers_len = {
            let mut buffer = [0; 1];
            bytes.read_exact(&mut buffer)?;
            u8::from_le_bytes(buffer)
        };
        let mut nullifiers = Vec::with_capacity(nullifiers_len as usize);
        for _ in 0..nullifiers_len {
            nullifiers.push(Hash(read_elems(&mut bytes)?));
        }
        Ok(Self {
            proof,
            tx_final,
            state_root_hash,
            nullifiers,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PayloadProof {
    Plonky2(Box<CompressedProof<F, C, D>>),
}

impl PayloadProof {
    pub fn write_bytes(&self, buffer: &mut Vec<u8>) {
        let PayloadProof::Plonky2(shrunk_main_pod_proof) = self;
        buffer
            .write_all(&[ProofType::Plonky2.to_byte()])
            .expect("byte write");
        plonky2::util::serialization::Write::write_compressed_proof(buffer, shrunk_main_pod_proof)
            .expect("vec write");
    }
    pub fn from_bytes(bytes: &[u8], common_data: &CommonCircuitData) -> Result<(Self, usize)> {
        let proof_type = ProofType::from_byte(&bytes[0])?;
        let bytes = &bytes[1..];
        let (proof, len): (Self, usize) = match proof_type {
            ProofType::Plonky2 => {
                let mut buffer = Buffer::new(bytes);
                let proof = plonky2::util::serialization::Read::read_compressed_proof(
                    &mut buffer,
                    common_data,
                )
                .map_err(|e| anyhow!("read_compressed_proof: {e}"))?;
                let len = buffer.pos();
                (PayloadProof::Plonky2(Box::new(proof)), len)
            }
        };

        // len+1 because at the beginning we used the first byte for the
        // proof_type
        Ok((proof, len + 1))
    }

    /// Construct a structurally-valid but unverifiable Plonky2 proof for test
    /// fixtures. The inner `CompressedProof` is hand-built with empty Merkle
    /// caps, empty openings, and an empty FRI proof — useful for mock parsers
    /// that need to return a `Payload` without paying the cost of generating
    /// a real proof.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn empty_for_test() -> Self {
        use plonky2::{
            field::polynomial::PolynomialCoeffs,
            fri::proof::{CompressedFriProof, CompressedFriQueryRounds},
            hash::merkle_tree::MerkleCap,
            plonk::proof::OpeningSet,
        };

        PayloadProof::Plonky2(Box::new(CompressedProof {
            wires_cap: MerkleCap::default(),
            plonk_zs_partial_products_cap: MerkleCap::default(),
            quotient_polys_cap: MerkleCap::default(),
            openings: OpeningSet::default(),
            opening_proof: CompressedFriProof {
                commit_phase_merkle_caps: Vec::new(),
                query_round_proofs: CompressedFriQueryRounds {
                    indices: Vec::new(),
                    // plonky2 uses hashbrown's HashMap here, not std's; defer
                    // to Default to avoid pulling in hashbrown as a direct dep.
                    initial_trees_proofs: Default::default(),
                    steps: Vec::new(),
                },
                final_poly: PolynomialCoeffs { coeffs: Vec::new() },
                pow_witness: F::ZERO,
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use plonky2::plonk::proof::CompressedProofWithPublicInputs;
    use pod2::{
        backends::plonky2::{
            basetypes::DEFAULT_VD_SET,
            mainpod::{Prover, public_inputs},
        },
        frontend::{MainPodBuilder, Operation},
        middleware::{
            Params, Statement, Value,
            containers::{Array, Set},
        },
    };

    use super::*;
    use crate::shrink::{ShrunkMainPodSetup, shrink_compress_pod};

    #[test]
    fn test_payload_roundtrip() {
        let params = Params::default();
        let vd_set = &*DEFAULT_VD_SET;
        let vds_root = vd_set.root();

        let input = r#"
        TxnFinalized(state_root_hash, tx_final, nullifiers) = AND(
            Equal(0, 0)
        )
        "#;
        let module = pod2::lang::load_module(input, "txn_finalized", &params, &[]).unwrap();
        let pred = module.predicate_ref_by_name("TxnFinalized").unwrap();

        println!("ShrunkMainPod setup");
        let shrunk_main_pod_build = ShrunkMainPodSetup::new(&params).build().unwrap();
        let common_data = &shrunk_main_pod_build.circuit_data.common;

        let payload = {
            let mut builder = MainPodBuilder::new(&params, vd_set);
            let tx_final = Value::from("dummy_tx_final");
            let nullifiers = vec![
                Hash(Value::from(1i64).raw().0),
                Hash(Value::from(2i64).raw().0),
                Hash(Value::from(3i64).raw().0),
            ];
            let nullifiers_set = Value::from(Set::new(HashSet::from_iter(
                nullifiers.iter().map(|h| Value::from(*h)),
            )));
            let state_root = Value::from("dummy_state_root");
            let st0 = builder.priv_op(Operation::eq(0, 0)).unwrap();
            let st_txn_finalized = builder
                .op(
                    true,
                    vec![
                        (0, state_root.clone()),
                        (1, tx_final.clone()),
                        (2, nullifiers_set.clone()),
                    ],
                    Operation::custom(pred.clone(), [st0]),
                )
                .unwrap();
            println!("st: {st_txn_finalized:?}");

            println!("MainPod prove");
            let prover = Prover {};
            let pod = builder.prove(&prover).unwrap();
            pod.pod.verify().unwrap();

            println!("MainPod shrink & compress");
            let shrunk_main_pod_proof =
                shrink_compress_pod(&shrunk_main_pod_build, pod.clone()).unwrap();

            Payload {
                proof: PayloadProof::Plonky2(Box::new(shrunk_main_pod_proof.clone())),
                tx_final: Hash(tx_final.raw().0),
                state_root_hash: Hash(state_root.raw().0),
                nullifiers: nullifiers.clone(),
            }
        };

        println!("Payload roundtrip");
        let payload_bytes = payload.to_bytes();
        let payload_decoded = Payload::from_bytes(&payload_bytes, common_data).unwrap();
        assert_eq!(payload, payload_decoded);

        println!("Verify shrunk mainPod");

        let nullifiers_set = Value::from(Set::new(HashSet::from_iter(
            payload.nullifiers.iter().map(|h| Value::from(*h)),
        )));
        let st = Statement::Custom(
            pred,
            vec![
                Value::from(payload.state_root_hash).into(),
                Value::from(payload.tx_final).into(),
                nullifiers_set.into(),
            ],
        );
        println!("st: {st:?}");

        let sts_root = Array::new(vec![Value::from(st.hash())]).commitment();
        let public_inputs = public_inputs(sts_root, vds_root, true);
        let PayloadProof::Plonky2(shrunk_main_pod_proof) = payload.proof;
        let proof_with_pis = CompressedProofWithPublicInputs {
            proof: *shrunk_main_pod_proof,
            public_inputs,
        };
        let proof = proof_with_pis
            .decompress(
                &shrunk_main_pod_build
                    .circuit_data
                    .verifier_only
                    .circuit_digest,
                &shrunk_main_pod_build.circuit_data.common,
            )
            .unwrap();
        shrunk_main_pod_build.circuit_data.verify(proof).unwrap();
    }
}
