//! Shared types + utilities used by `synchronizer` and `relayer`.
//!
//! Post-migration: depends on `txlib-core` for the SHA-256 commitment scheme
//! and `risc0-zkvm` for receipt verification. No pod2 / plonky2 anywhere.

pub mod blob;
pub mod payload;
pub mod proof;

use std::io;

use anyhow::Result;
use tracing_subscriber::{EnvFilter, fmt::time::OffsetTime, prelude::*};

pub use txlib_core::Hash;

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

/// `0x`-prefixed lowercase hex.
pub fn encode_hash_hex(hash: &Hash) -> String {
    let mut s = String::with_capacity(2 + 64);
    s.push_str("0x");
    for b in hash.as_bytes() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub fn log_init() {
    let timer = time::format_description::parse("[hour]:[minute]:[second].[subsecond digits:2]")
        .expect("valid format");
    let time_offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    let timer = OffsetTime::new(time_offset, timer);
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_timer(timer))
        .with(EnvFilter::from_default_env())
        .init();
}
