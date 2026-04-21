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
    /// Commitment of the finalized transaction dictionary `{live, nullifiers, tx_start, tx_end}`.
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
    Groth16(Vec<u8>),
}

impl PayloadProof {
    pub fn write_bytes(&self, buffer: &mut Vec<u8>) {
        match self {
            PayloadProof::Plonky2(shrunk_main_pod_proof) => {
                buffer
                    .write_all(&[ProofType::Plonky2.to_byte()])
                    .expect("byte write");
                plonky2::util::serialization::Write::write_compressed_proof(
                    buffer,
                    shrunk_main_pod_proof,
                )
                .expect("vec write");
            }
            PayloadProof::Groth16(b) => {
                buffer
                    .write_all(&[ProofType::Groth16.to_byte()])
                    .expect("byte write");
                buffer
                    .write_all(&b.len().to_le_bytes())
                    .expect("g16 proof bytes length write");
                buffer.write_all(b).expect("g16 proof bytes write");
            }
        }
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
            ProofType::Groth16 => {
                // get the length
                let len_bytes: [u8; 8] = bytes[0..8].try_into()?;
                let len: usize = u64::from_le_bytes(len_bytes) as usize;
                // return the rest of bytes of the Groth16 proof
                (PayloadProof::Groth16(bytes[8..8 + len].to_vec()), 8 + len)
            }
        };

        // len+1 because at the beginning we used the first byte for the
        // proof_type
        Ok((proof, len + 1))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use plonky2::plonk::proof::CompressedProofWithPublicInputs;
    use pod2::{
        backends::plonky2::{
            basetypes::DEFAULT_VD_SET,
            mainpod::{Prover, calculate_statements_hash},
        },
        frontend::{MainPodBuilder, Operation},
        middleware::{Params, Statement, Value, containers::Set},
    };

    use super::*;
    use crate::shrink::{ShrunkMainPodSetup, shrink_compress_pod};

    #[test]
    fn test_payload_roundtrip() {
        let params = Params::default();
        let vd_set = &*DEFAULT_VD_SET;
        let vds_root = vd_set.root();

        let input = r#"
        TxnFinalized(tx_final, nullifiers, state_root) = AND(
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
                        (0, tx_final.clone()),
                        (1, nullifiers_set.clone()),
                        (2, state_root.clone()),
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

        let sts_hash = calculate_statements_hash(&[st.clone().into()]);
        let public_inputs = [sts_hash.0, vds_root.0].concat();
        let shrunk_main_pod_proof = match payload.proof {
            PayloadProof::Plonky2(proof) => proof,
            PayloadProof::Groth16(_) => todo!(),
        };
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
