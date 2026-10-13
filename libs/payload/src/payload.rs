use std::io::{Read, Write};

use anyhow::{Result, anyhow};
use plonky2::{
    field::types::{Field, Field64, PrimeField64},
    plonk::proof::CompressedProof,
    util::serialization::Buffer,
};
use pod2::middleware::{
    C, CommonCircuitData, CustomPredicateRef, D, F, Hash, Statement, Value, containers::Set,
};
use txlib::Tx;

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

const MAX_HASHES_PER_LIST: usize = u8::MAX as usize;

/// A list of hashes, length-prefixed by a single `u8` count: at most 255
/// entries.
fn write_hashes(buffer: &mut Vec<u8>, hashes: &[Hash]) -> Result<()> {
    let count = u8::try_from(hashes.len())
        .map_err(|_| anyhow!("hash list exceeds payload limit of {MAX_HASHES_PER_LIST}"))?;
    buffer.write_all(&count.to_le_bytes()).expect("vec write");
    for hash in hashes {
        write_elems(buffer, &hash.0);
    }
    Ok(())
}

fn read_hashes(bytes: &mut &[u8]) -> Result<Vec<Hash>> {
    let len = {
        let mut buffer = [0; 1];
        bytes.read_exact(&mut buffer)?;
        u8::from_le_bytes(buffer)
    };
    let mut hashes = Vec::with_capacity(len as usize);
    for _ in 0..len {
        hashes.push(Hash(read_elems(bytes)?));
    }
    Ok(hashes)
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub struct Payload {
    pub proof: PayloadProof,
    /// Commitment of the finalized transaction dictionary.
    pub tx_final: Hash,
    pub state_root: Hash,
    pub nullifiers: Vec<Hash>,
    /// Commitments of every state produced, including intermediate states.
    /// Reconstructed as a set when verifying the public transaction statement.
    pub created: Vec<Hash>,
}

const PAYLOAD_MAGIC: u16 = 0xd10b;

impl Payload {
    pub fn from_tx(proof: PayloadProof, tx: &Tx, state_root: Hash) -> Result<Self> {
        let nullifiers = tx.nullifier_hashes()?;
        let created = tx.created_commitments()?;
        Self::validate_counts(nullifiers.len(), created.len())?;
        Ok(Self {
            proof,
            tx_final: tx.dict().commitment(),
            state_root,
            nullifiers,
            created,
        })
    }

    /// Check the wire-format capacities before generating a transaction proof.
    pub fn validate_tx(tx: &Tx) -> Result<()> {
        Self::validate_counts(
            tx.nullifier_hashes()?.len(),
            tx.created_commitments()?.len(),
        )
    }

    fn validate_counts(nullifiers: usize, created: usize) -> Result<()> {
        for (name, count) in [("nullifiers", nullifiers), ("created states", created)] {
            if count > MAX_HASHES_PER_LIST {
                return Err(anyhow!(
                    "transaction has {count} {name}; payload limit is {MAX_HASHES_PER_LIST}"
                ));
            }
        }
        Ok(())
    }

    pub fn finalized_statement(&self, predicate: CustomPredicateRef) -> Statement {
        let hashes_to_set =
            |hashes: &[Hash]| Set::new(hashes.iter().map(|hash| Value::from(*hash)).collect());
        Statement::Custom(
            predicate,
            vec![
                Value::from(self.state_root).into(),
                Value::from(self.tx_final).into(),
                Value::from(hashes_to_set(&self.nullifiers)).into(),
                Value::from(hashes_to_set(&self.created)).into(),
            ],
        )
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Self::validate_counts(self.nullifiers.len(), self.created.len())?;
        let mut buffer = Vec::new();
        buffer
            .write_all(&PAYLOAD_MAGIC.to_le_bytes())
            .expect("vec write");
        self.proof.write_bytes(&mut buffer);
        write_elems(&mut buffer, &self.tx_final.0);
        write_elems(&mut buffer, &self.state_root.0);
        write_hashes(&mut buffer, &self.nullifiers)?;
        write_hashes(&mut buffer, &self.created)?;
        Ok(buffer)
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
        let state_root = Hash(read_elems(&mut bytes)?);
        let nullifiers = read_hashes(&mut bytes)?;
        let created = read_hashes(&mut bytes)?;
        Ok(Self {
            proof,
            tx_final,
            state_root,
            nullifiers,
            created,
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
    fn hash_lists_preserve_one_byte_count_at_capacity() {
        let hashes = vec![pod2::middleware::EMPTY_HASH; MAX_HASHES_PER_LIST];
        let mut encoded = Vec::new();
        write_hashes(&mut encoded, &hashes).unwrap();
        assert_eq!(encoded[0], u8::MAX);
        assert_eq!(encoded.len(), 1 + 32 * MAX_HASHES_PER_LIST);
        let mut remaining = encoded.as_slice();
        assert_eq!(read_hashes(&mut remaining).unwrap(), hashes);
        assert!(remaining.is_empty());
    }

    #[test]
    fn payload_from_tx_enforces_capacity_for_created_and_nullifiers() {
        use pod2::middleware::{EMPTY_HASH, containers::Dictionary};
        use std::sync::Arc;
        let hashes = Set::new(
            (0..=MAX_HASHES_PER_LIST)
                .map(|index| Value::from(index as i64))
                .collect(),
        );
        let mut tx = Tx {
            created: hashes.clone(),
            nullifiers: hashes,
            ctx: Dictionary::new(Default::default()),
            state_header: Arc::new(txlib::StateHeader::new(
                0, 0, EMPTY_HASH, EMPTY_HASH, EMPTY_HASH, EMPTY_HASH,
            )),
        };
        for index in 0..2 {
            assert!(
                Payload::validate_tx(&tx)
                    .unwrap_err()
                    .to_string()
                    .contains("payload limit is 255")
            );
            let error =
                Payload::from_tx(PayloadProof::empty_for_test(), &tx, EMPTY_HASH).unwrap_err();
            assert!(error.to_string().contains("payload limit is 255"));
            let oversized = if index == 0 {
                &mut tx.nullifiers
            } else {
                &mut tx.created
            };
            assert!(error.to_string().contains(if index == 0 {
                "nullifiers"
            } else {
                "created states"
            }));
            oversized.delete(&Value::from(0_i64)).unwrap();
        }
        Payload::validate_tx(&tx).unwrap();
        let payload = Payload::from_tx(PayloadProof::empty_for_test(), &tx, EMPTY_HASH).unwrap();
        assert_eq!(payload.created.len(), MAX_HASHES_PER_LIST);
        assert_eq!(payload.nullifiers.len(), MAX_HASHES_PER_LIST);
    }

    #[test]
    fn serialization_checks_capacity_for_directly_constructed_payloads() {
        use pod2::middleware::EMPTY_HASH;
        let mut payload = Payload {
            proof: PayloadProof::empty_for_test(),
            tx_final: EMPTY_HASH,
            state_root: EMPTY_HASH,
            nullifiers: vec![EMPTY_HASH; MAX_HASHES_PER_LIST],
            created: vec![EMPTY_HASH; MAX_HASHES_PER_LIST],
        };
        payload.to_bytes().unwrap();
        payload.nullifiers.push(EMPTY_HASH);
        assert!(
            payload
                .to_bytes()
                .unwrap_err()
                .to_string()
                .contains("256 nullifiers")
        );
        payload.nullifiers.pop();
        payload.created.push(EMPTY_HASH);
        assert!(
            payload
                .to_bytes()
                .unwrap_err()
                .to_string()
                .contains("256 created states")
        );

        let mut bytes = Vec::new();
        assert!(write_hashes(&mut bytes, &payload.created).is_err());
        assert!(bytes.is_empty());
    }

    #[test]
    #[ignore = "generates and shrinks a real Plonky2 proof"]
    fn test_payload_roundtrip() {
        let params = Params::default();
        let vd_set = &*DEFAULT_VD_SET;
        let vds_root = vd_set.root();

        let input = r#"
        TxnFinalized(state_root, tx_final, nullifiers, created) = AND(
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
            let created = vec![
                Hash(Value::from(10i64).raw().0),
                Hash(Value::from(11i64).raw().0),
            ];
            let created_set = Value::from(Set::new(HashSet::from_iter(
                created.iter().map(|h| Value::from(*h)),
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
                        (3, created_set.clone()),
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
                state_root: Hash(state_root.raw().0),
                nullifiers: nullifiers.clone(),
                created: created.clone(),
            }
        };

        println!("Payload roundtrip");
        let payload_bytes = payload.to_bytes().unwrap();
        let payload_decoded = Payload::from_bytes(&payload_bytes, common_data).unwrap();
        assert_eq!(payload, payload_decoded);

        println!("Verify shrunk mainPod");

        let nullifiers_set = Value::from(Set::new(HashSet::from_iter(
            payload.nullifiers.iter().map(|h| Value::from(*h)),
        )));
        let created_set = Value::from(Set::new(HashSet::from_iter(
            payload.created.iter().map(|h| Value::from(*h)),
        )));
        let st = Statement::Custom(
            pred,
            vec![
                Value::from(payload.state_root).into(),
                Value::from(payload.tx_final).into(),
                nullifiers_set.into(),
                created_set.into(),
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
