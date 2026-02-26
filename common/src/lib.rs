//! `common` contains shared logic across the various crates of the project.

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
///   B) "shrink":
///     first shrinks the given MainPod's proof, and then compresses it,
///     returning the compressed proof (without public inputs)
pub mod shrink;

#[cfg(not(feature = "groth16"))]
pub mod groth {
    use anyhow::Result;
    pub fn load_vk() -> Result<()> {
        panic!("groth16 disabled");
    }
}

use std::io;

use anyhow::{Result, anyhow};
use pod2::middleware::{Value, containers};
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

pub fn set_from_value(v: &Value) -> Result<containers::Set> {
    match v.typed() {
        pod2::middleware::TypedValue::Set(s) => Ok(s.clone()),
        _ => Err(anyhow!("Invalid set")),
    }
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
