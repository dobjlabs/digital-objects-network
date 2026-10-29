use anyhow::{Result, anyhow, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use payload::{db_bytes_to_hash, hash_to_db_bytes};
use pod2::{
    backends::plonky2::primitives::merkletree::{self, MerkleProof},
    middleware::{Hash, RawValue, Value},
};
use serde::{Deserialize, Serialize};
use txlib::StateHeader;

const FORMAT_VERSION: u8 = 1;
const HASH_BYTES: usize = 32;
const I64_BYTES: usize = 8;
/// Everything before the siblings: the version byte, the state header and
/// the array index.
const FIXED_BYTES: usize = 1 + 2 * I64_BYTES + 4 * HASH_BYTES + I64_BYTES;

/// Proof that an object's commitment sits at `index` of the created array
/// committed to by `state_header`, whose hash is a state root.
///
/// Encoded as base64 of: version byte, block number, block timestamp, block
/// hash, created root, nullifiers root, prior state history root, index,
/// then the Merkle siblings from the root down. Integers are little-endian
/// i64 and hashes 32 little-endian limb bytes.
#[derive(Debug, Clone)]
pub(crate) struct CreationProof {
    pub(crate) state_header: StateHeader,
    pub(crate) index: i64,
    pub(crate) proof: MerkleProof,
}

/// Field-for-field copy of pod2's `MerkleProof`, whose fields are private to
/// its crate; the two convert through their shared serde form.
#[derive(Serialize, Deserialize)]
struct MerkleProofFields {
    existence: bool,
    siblings: Vec<Hash>,
    other_leaf: Option<(RawValue, RawValue)>,
}

/// Reads fields front to back from bytes whose length is already checked.
struct ByteReader<'a>(&'a [u8]);

impl ByteReader<'_> {
    fn take(&mut self, len: usize) -> &[u8] {
        let (head, tail) = self.0.split_at(len);
        self.0 = tail;
        head
    }

    fn i64(&mut self) -> i64 {
        i64::from_le_bytes(self.take(I64_BYTES).try_into().expect("8 bytes"))
    }

    fn hash(&mut self) -> Result<Hash> {
        db_bytes_to_hash(self.take(HASH_BYTES))
    }
}

impl CreationProof {
    pub(crate) fn encode(&self) -> Result<String> {
        let fields: MerkleProofFields = serde_json::from_value(serde_json::to_value(&self.proof)?)?;
        let header = &self.state_header;
        let mut bytes = Vec::with_capacity(FIXED_BYTES + fields.siblings.len() * HASH_BYTES);
        bytes.push(FORMAT_VERSION);
        bytes.extend_from_slice(&header.block_number.to_le_bytes());
        bytes.extend_from_slice(&header.block_timestamp.to_le_bytes());
        for hash in [
            header.block_hash,
            header.created_root,
            header.nullifiers_root,
            header.prior_state_history_root,
        ] {
            bytes.extend(hash_to_db_bytes(hash));
        }
        bytes.extend_from_slice(&self.index.to_le_bytes());
        for sibling in fields.siblings {
            bytes.extend(hash_to_db_bytes(sibling));
        }
        Ok(STANDARD.encode(bytes))
    }

    pub(crate) fn decode(encoded: &str) -> Result<Self> {
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|err| anyhow!("creation proof is not base64: {err}"))?;
        ensure!(
            bytes.len() >= FIXED_BYTES && (bytes.len() - FIXED_BYTES).is_multiple_of(HASH_BYTES),
            "creation proof has an invalid length of {} bytes",
            bytes.len()
        );
        ensure!(
            bytes[0] == FORMAT_VERSION,
            "creation proof has unknown version {}",
            bytes[0]
        );
        let mut reader = ByteReader(&bytes[1..]);
        let block_number = reader.i64();
        let block_timestamp = reader.i64();
        let state_header = StateHeader::new(
            block_number,
            block_timestamp,
            reader.hash()?,
            reader.hash()?,
            reader.hash()?,
            reader.hash()?,
        );
        let index = reader.i64();
        let siblings = reader
            .0
            .chunks(HASH_BYTES)
            .map(db_bytes_to_hash)
            .collect::<Result<Vec<_>>>()?;
        let proof = serde_json::from_value(serde_json::to_value(MerkleProofFields {
            existence: true,
            siblings,
            other_leaf: None,
        })?)?;
        Ok(Self {
            state_header,
            index,
            proof,
        })
    }

    /// Checks that the proof places `commitment` in the header's created
    /// array, and returns the state root the header hashes to. Whether that
    /// root is one the chain reached is for the caller to check.
    pub(crate) fn verify(&self, commitment: Hash) -> Result<Hash> {
        merkletree::verify(
            self.state_header.created_root,
            &self.proof,
            &Value::from(self.index).raw(),
            &Value::from(commitment).raw(),
        )
        .map_err(|err| anyhow!("creation proof does not prove the object's creation: {err}"))?;
        Ok(self.state_header.hash())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use payload::test_state::TestState;
    use pod2::middleware::F;

    fn commitment(seed: u64) -> Hash {
        Hash([F(seed), F(1), F(2), F(3)])
    }

    fn proof_for(state: &TestState, commitment: Hash) -> CreationProof {
        let (created_root, nullifiers_root, prior_state_history_root) = state.roots();
        let (index, proof) = state.created_membership_proof(commitment);
        CreationProof {
            state_header: StateHeader::new(
                state.block_number,
                state.block_timestamp,
                state.block_hash,
                created_root,
                nullifiers_root,
                prior_state_history_root,
            ),
            index,
            proof,
        }
    }

    #[test]
    fn a_decoded_proof_verifies_against_the_state_root() {
        let mut state = TestState::default();
        state.apply_tx((0..5).map(commitment), []);
        let proof = proof_for(&state, commitment(3));
        let encoded = proof.encode().unwrap();
        let decoded = CreationProof::decode(&encoded).unwrap();
        assert_eq!(decoded.encode().unwrap(), encoded);
        assert_eq!(
            decoded.verify(commitment(3)).unwrap(),
            proof.state_header.hash()
        );
        assert!(decoded.verify(commitment(4)).is_err());
    }

    #[test]
    fn malformed_proofs_do_not_decode() {
        let mut state = TestState::default();
        state.apply_tx((0..5).map(commitment), []);
        let bytes = STANDARD
            .decode(proof_for(&state, commitment(0)).encode().unwrap())
            .unwrap();

        let truncated = STANDARD.encode(&bytes[..bytes.len() - 1]);
        let err = CreationProof::decode(&truncated).unwrap_err();
        assert!(err.to_string().contains("invalid length"), "{err}");

        let mut other_version = bytes.clone();
        other_version[0] = FORMAT_VERSION + 1;
        let err = CreationProof::decode(&STANDARD.encode(other_version)).unwrap_err();
        assert!(err.to_string().contains("unknown version"), "{err}");
    }
}
