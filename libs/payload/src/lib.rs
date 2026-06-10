//! `common` contains shared logic across the various crates of the project.

pub mod blob;
pub mod payload;
pub mod proof;
/// POD proving uses plonky2's "shrink" path: first shrinks the given MainPod's
/// proof, then compresses it, returning the compressed proof (without public
/// inputs). See the plonky2 docs for details:
/// <https://github.com/0xPARC/plonky2/blob/109d517d09c210ae4c2cee381d3e3fbc04aa3812/plonky2/src/plonk/proof.rs#L58>.
pub mod shrink;
#[cfg(any(test, feature = "test-utils"))]
pub mod test_state;

use std::io;

use anyhow::{Result, anyhow};
use hex::{FromHex, ToHex};
use pod2::middleware::{F, Hash, RawValue};
use tracing_subscriber::{EnvFilter, fmt::time::OffsetTime, prelude::*};

pub fn load_dotenv() -> Result<()> {
    for filename in [".env.default", ".env"] {
        if let Err(err) = dotenvy::from_filename_override(filename) {
            match err {
                dotenvy::Error::Io(e) if e.kind() == io::ErrorKind::NotFound => {}
                _ => return Err(err)?,
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProofType {
    Plonky2,
}
impl std::str::FromStr for ProofType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "plonky2" => Ok(ProofType::Plonky2),
            _ => Err(anyhow!("unsupported PROOF_TYPE {s}")),
        }
    }
}

impl ProofType {
    pub fn from_byte(input: &u8) -> Result<ProofType> {
        match input {
            0u8 => Ok(ProofType::Plonky2),
            _ => Err(anyhow!("unsupported PROOF_TYPE {input}")),
        }
    }
    pub fn to_byte(self) -> u8 {
        match self {
            ProofType::Plonky2 => 0u8,
        }
    }
}

pub fn encode_hash_hex(hash: &Hash) -> String {
    format!("0x{}", hash.encode_hex::<String>())
}

pub fn decode_hash_hex(s: &str) -> Result<Hash> {
    let trimmed = s.strip_prefix("0x").unwrap_or(s);
    Hash::from_hex(trimmed).map_err(|err| anyhow!("invalid hash hex {s:?}: {err}"))
}

/// Encode a `Hash` as the little-endian limb bytes stored in `BYTEA` columns.
pub fn hash_to_db_bytes(hash: Hash) -> Vec<u8> {
    RawValue::from(hash).to_bytes()
}

/// Decode the little-endian limb bytes from a `BYTEA` column back into a `Hash`.
pub fn db_bytes_to_hash(bytes: &[u8]) -> Result<Hash> {
    let limbs: [[u8; 8]; 4] = bytes
        .chunks_exact(8)
        .map(|chunk| {
            chunk
                .try_into()
                .map_err(|_| anyhow!("invalid hash limb length"))
        })
        .collect::<Result<Vec<[u8; 8]>>>()?
        .try_into()
        .map_err(|_| anyhow!("invalid hash byte length"))?;

    Ok(Hash(limbs.map(|limb| F(u64::from_le_bytes(limb)))))
}

pub fn log_init() {
    // Full date: `[year]-[month padding:zero]-[day padding:zero]`
    let timer = time::format_description::parse("[hour]:[minute]:[second].[subsecond digits:2]")
        .expect("valid format");
    let time_offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    let timer = OffsetTime::new(time_offset, timer);
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_timer(timer))
        .with(EnvFilter::from_default_env())
        .init();
}
