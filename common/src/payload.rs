//! On-chain payload format.
//!
//! Wire layout for one EIP-4844 blob (after `blob::decode_simple_blob`):
//! ```text
//! | magic (2 bytes LE = 0xd10b) | bincode-encoded risc0 Receipt (rest) |
//! ```
//!
//! The proof header (`tx_final`, `state_root_hash`, `nullifiers`) lives
//! inside the receipt's journal as a borsh-encoded
//! [`txlib_core::abi::GuestJournal`]. The synchronizer learns these values
//! by verifying the receipt and decoding the journal — no redundant header
//! is carried in the blob.
//!
//! [`Payload`] is the *parsed* representation: it holds the journal-derived
//! `(tx_final, state_root_hash, nullifiers)` plus the raw receipt bytes for
//! diagnostics. Constructing one is the result of a successful
//! [`crate::proof::Risc0Verifier::parse_blob`] call.

use anyhow::{Result, anyhow};
use txlib_core::Hash;

pub const PAYLOAD_MAGIC: u16 = 0xd10b;

/// A successfully parsed + verified blob payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payload {
    pub tx_final: Hash,
    pub state_root_hash: Hash,
    pub nullifiers: Vec<Hash>,
    /// Raw bincode-serialized risc0 `Receipt`. Held for diagnostics and
    /// re-broadcast; verification has already happened by the time you
    /// have a `Payload`.
    pub receipt_bytes: Vec<u8>,
}

/// Wrap raw receipt bytes in the magic envelope.
pub fn encode_blob_payload(receipt_bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + receipt_bytes.len());
    out.extend_from_slice(&PAYLOAD_MAGIC.to_le_bytes());
    out.extend_from_slice(receipt_bytes);
    out
}

/// Strip the magic envelope and return the receipt bytes.
///
/// Returns `Ok(None)` if the magic doesn't match — these bytes belong to
/// some other application's blob and should be silently skipped.
/// Returns `Err` only on malformed input (too short).
pub fn decode_blob_envelope(bytes: &[u8]) -> Result<Option<&[u8]>> {
    if bytes.len() < 2 {
        return Err(anyhow!(
            "blob payload too short: {} bytes (need at least 2 for magic)",
            bytes.len()
        ));
    }
    let magic = u16::from_le_bytes([bytes[0], bytes[1]]);
    if magic != PAYLOAD_MAGIC {
        return Ok(None);
    }
    Ok(Some(&bytes[2..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_roundtrip() {
        let receipt = vec![0xde, 0xad, 0xbe, 0xef];
        let blob = encode_blob_payload(&receipt);
        assert_eq!(blob.len(), 2 + receipt.len());
        let decoded = decode_blob_envelope(&blob).unwrap().unwrap();
        assert_eq!(decoded, receipt.as_slice());
    }

    #[test]
    fn empty_receipt_envelope() {
        let blob = encode_blob_payload(&[]);
        assert_eq!(blob.len(), 2);
        let decoded = decode_blob_envelope(&blob).unwrap().unwrap();
        assert_eq!(decoded, &[] as &[u8]);
    }

    #[test]
    fn wrong_magic_yields_none() {
        let blob = vec![0x00, 0x00, 0xff, 0xff];
        assert!(decode_blob_envelope(&blob).unwrap().is_none());
    }

    #[test]
    fn too_short_errors() {
        assert!(decode_blob_envelope(&[0x0b]).is_err());
        assert!(decode_blob_envelope(&[]).is_err());
    }
}
