use anyhow::{Context, Result};
use hex::FromHex;
use pod2::middleware::Hash;
use serde::Deserialize;

pub struct TxnFinalized {
    pub tx_hash: Hash,
    pub nullifiers: Vec<Hash>,
    pub state_root_hash: Hash,
}

pub trait ProofParser: Send + Sync {
    fn parse_blob(&self, blob_bytes: &[u8]) -> Result<Option<TxnFinalized>>;
}

/// Mock parser: accepts JSON `{ "tx_hash": "0x...", "nullifiers": [...], "state_root_hash": "0x..." }`
/// Hash bytes serialized as lowercase hex strings (with or without "0x" prefix).
pub struct MockProofParser;

#[derive(Deserialize)]
struct MockProofJson {
    tx_hash: String,
    nullifiers: Vec<String>,
    state_root_hash: String,
}

pub fn parse_hex_hash(s: &str) -> Result<Hash> {
    let hex = s.strip_prefix("0x").unwrap_or(s);
    Hash::from_hex(hex).context("Invalid Hash hex")
}

impl ProofParser for MockProofParser {
    fn parse_blob(&self, blob_bytes: &[u8]) -> Result<Option<TxnFinalized>> {
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
        Ok(Some(TxnFinalized {
            tx_hash,
            nullifiers,
            state_root_hash,
        }))
    }
}

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
        let parser = MockProofParser;

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
        let txn = result.unwrap();
        assert_eq!(txn.tx_hash, tx_hash);
        assert_eq!(txn.nullifiers.len(), 1);
        assert_eq!(txn.nullifiers[0], nullifier);
        assert_eq!(txn.state_root_hash, state_root);
    }

    #[test]
    fn test_mock_proof_parser_invalid_returns_none() {
        let parser = MockProofParser;
        let result = parser.parse_blob(b"not json").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_mock_proof_parser_no_prefix() {
        let parser = MockProofParser;

        let tx_hash = EMPTY_HASH;
        let tx_hex = hash_to_hex(&tx_hash); // no "0x" prefix

        let json = format!(
            r#"{{"tx_hash":"{}","nullifiers":[],"state_root_hash":"{}"}}"#,
            tx_hex, tx_hex
        );

        let result = parser.parse_blob(json.as_bytes()).unwrap();
        assert!(result.is_some());
        let txn = result.unwrap();
        assert_eq!(txn.tx_hash, tx_hash);
    }
}
