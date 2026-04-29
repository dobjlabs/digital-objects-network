//! SHA-256 hashing primitives with domain separation.

use alloc::format;
use alloc::string::String;
use borsh::{BorshDeserialize, BorshSerialize};
use core::fmt;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

/// 32-byte SHA-256 digest. JSON-serialized as a `0x`-prefixed lowercase hex string.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
#[repr(transparent)]
pub struct Hash(pub [u8; 32]);

pub const ZERO_HASH: Hash = Hash([0u8; 32]);

impl Hash {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub const fn into_bytes(self) -> [u8; 32] {
        self.0
    }

    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 32]
    }
}

impl Default for Hash {
    fn default() -> Self {
        ZERO_HASH
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x")?;
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl Serialize for Hash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = String::with_capacity(2 + 64);
        s.push_str("0x");
        for b in &self.0 {
            s.push_str(&format!("{b:02x}"));
        }
        serializer.serialize_str(&s)
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct HashVisitor;
        impl serde::de::Visitor<'_> for HashVisitor {
            type Value = Hash;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a 32-byte hex string, optionally `0x`-prefixed")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Hash, E> {
                let s = v.strip_prefix("0x").unwrap_or(v);
                if s.len() != 64 {
                    return Err(E::invalid_length(s.len(), &"64 hex chars"));
                }
                let mut out = [0u8; 32];
                for i in 0..32 {
                    out[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)
                        .map_err(|e| E::custom(format!("hex parse error: {e}")))?;
                }
                Ok(Hash(out))
            }
        }
        deserializer.deserialize_str(HashVisitor)
    }
}

/// SHA-256 of a byte slice.
pub fn sha256(input: &[u8]) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(input);
    let out: [u8; 32] = hasher.finalize().into();
    Hash(out)
}

/// SHA-256 over the concatenation of `parts` without an intermediate allocation.
pub fn sha256_concat(parts: &[&[u8]]) -> Hash {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    let out: [u8; 32] = hasher.finalize().into();
    Hash(out)
}

/// 8-byte domain separators prepended to the content of every distinct hash.
///
/// All hashes in this crate route through one of these domains. Adding a new
/// domain means choosing a fresh 8-byte tag, never reusing one — that's what
/// keeps two different hash recipes from ever colliding by construction.
pub mod domain {
    pub const OBJECT: &[u8; 8] = b"DOBJ-OBJ";
    pub const TX_FINAL: &[u8; 8] = b"DOBJ-TXF";
    pub const STATE_ROOT: &[u8; 8] = b"DOBJ-STR";
    pub const NULLIFIER_KEY: &[u8; 8] = b"DOBJ-NUK";
    pub const NULLIFIER: &[u8; 8] = b"DOBJ-NUL";
    pub const SMT_LEAF: &[u8; 8] = b"DOBJ-SLF";
    pub const SMT_NODE: &[u8; 8] = b"DOBJ-SND";
    pub const VALUE_BYTES: &[u8; 8] = b"DOBJ-VBT";
    pub const VALUE_INT: &[u8; 8] = b"DOBJ-VIN";
    pub const VALUE_STR: &[u8; 8] = b"DOBJ-VST";
    pub const VALUE_HASH: &[u8; 8] = b"DOBJ-VHA";
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn known_sha256() {
        // sha256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        let h = sha256(b"abc");
        assert_eq!(
            &h.0,
            &[
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
                0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
                0xf2, 0x00, 0x15, 0xad,
            ]
        );
    }

    #[test]
    fn sha256_concat_matches_concat() {
        let h1 = sha256(b"hello world");
        let h2 = sha256_concat(&[b"hello", b" ", b"world"]);
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_serde_roundtrip() {
        let h = sha256(b"some bytes");
        let json = serde_json::to_string(&h).unwrap();
        assert!(json.starts_with("\"0x"));
        let h2: Hash = serde_json::from_str(&json).unwrap();
        assert_eq!(h, h2);
    }

    #[test]
    fn hash_borsh_roundtrip() {
        let h = sha256(b"borsh");
        let bytes = borsh::to_vec(&h).unwrap();
        assert_eq!(bytes.len(), 32);
        let h2: Hash = borsh::from_slice(&bytes).unwrap();
        assert_eq!(h, h2);
    }

    #[test]
    fn zero_is_default() {
        assert!(ZERO_HASH.is_zero());
        assert_eq!(Hash::default(), ZERO_HASH);
    }

    #[test]
    fn debug_format_is_0x_lowercase_hex() {
        let h = Hash([0xab; 32]);
        let s = format!("{h:?}");
        assert_eq!(s, alloc::format!("0x{}", "ab".repeat(32)));
    }

    #[test]
    fn deserialize_accepts_no_prefix() {
        let bytes = vec![0xcd; 32];
        let json = format!("\"{}\"", hex_encode(&bytes));
        let h: Hash = serde_json::from_str(&json).unwrap();
        assert_eq!(h.0, bytes.as_slice());
    }

    fn hex_encode(b: &[u8]) -> String {
        let mut s = String::with_capacity(b.len() * 2);
        for byte in b {
            s.push_str(&format!("{byte:02x}"));
        }
        s
    }
}
