pub use common::proof::{BlobParser, ProofParser};

#[cfg(test)]
use anyhow::Result;
#[cfg(test)]
use common::payload::{Payload, PayloadProof};
#[cfg(test)]
use pod2::middleware::Hash;

#[cfg(test)]
#[derive(serde::Deserialize)]
struct MockProofJson {
    tx_final: String,
    nullifiers: Vec<String>,
    state_root_hash: String,
}

#[cfg(test)]
pub fn parse_hex_hash(s: &str) -> Result<Hash> {
    use anyhow::Context;

    let hex = s.strip_prefix("0x").unwrap_or(s);
    <Hash as hex::FromHex>::from_hex(hex).context("Invalid Hash hex")
}

/// Test-only parser for unit tests that don't need real cryptographic verification.
#[cfg(test)]
pub struct MockBlobParser;

#[cfg(test)]
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
        let tx_hex = hash_to_hex(&tx_final);

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
