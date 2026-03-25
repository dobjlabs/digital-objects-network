//! `common` contains shared logic across the various crates of the project.

pub mod blob;
/// POD proving:
/// 2 options to prepare the POD proofs:
///   A) "groth":
///     first compute the one extra recursive proof from the given MainPod's proof in
///     order to shrink it, together with using the bn254's poseidon variant in the
///     configuration of the plonky2 prover, in order to make it compatible with the
///     Groth16 circuit.
///     Then compute a Groth16 proof which verifies the last plonky2 proof
#[cfg(feature = "groth16")]
pub mod groth;
pub mod payload;
pub mod proof;
///   B) "shrink":
///     first shrinks the given MainPod's proof, and then compresses it,
///     returning the compressed proof (without public inputs). this is a plonky2 specific optimization,
///     see the plonky2 documentation for details: https://github.com/0xPARC/plonky2/blob/109d517d09c210ae4c2cee381d3e3fbc04aa3812/plonky2/src/plonk/proof.rs#L58
pub mod shrink;
#[cfg(any(test, feature = "test-utils"))]
pub mod test_state;

#[cfg(not(feature = "groth16"))]
pub mod groth {
    use anyhow::Result;
    pub fn load_vk() -> Result<()> {
        panic!("groth16 disabled");
    }
}

use std::io;

use anyhow::{Result, anyhow};
use hex::ToHex;
use pod2::middleware::Hash;
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
    Groth16,
}
impl std::str::FromStr for ProofType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "plonky2" => Ok(ProofType::Plonky2),
            "groth16" => Ok(ProofType::Groth16),
            _ => Err(anyhow!("unsupported PROOF_TYPE {s}")),
        }
    }
}

impl ProofType {
    pub fn from_byte(input: &u8) -> Result<ProofType> {
        match input {
            0u8 => Ok(ProofType::Plonky2),
            1u8 => Ok(ProofType::Groth16),
            _ => Err(anyhow!("unsupported PROOF_TYPE {input}")),
        }
    }
    pub fn to_byte(self) -> u8 {
        match self {
            ProofType::Plonky2 => 0u8,
            ProofType::Groth16 => 1u8,
        }
    }
}

pub fn encode_hash_hex(hash: &Hash) -> String {
    format!("0x{}", hash.encode_hex::<String>())
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
