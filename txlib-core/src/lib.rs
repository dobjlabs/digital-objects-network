//! Core data types and commitment scheme for the risc0-based zk-craft stack.
//!
//! Shared by the risc0 guest (no_std) and host code (synchronizer / driver).
//! Defines:
//! - [`hash`]    — SHA-256 hashing with domain separation, [`Hash`] type
//! - [`value`]   — typed [`Value`] for object fields
//! - [`object`]  — [`Object`] with deterministic commitment
//! - [`tx`]      — [`Tx`], [`StateRoot`], `tx_final`, nullifier derivation
//! - [`merkle`]  — sparse Merkle tree (depth 256) verification + builder
//! - [`abi`]     — [`GuestInput`] / [`GuestJournal`] host↔guest contract
//!
//! All hashing is SHA-256 (no Poseidon, no Goldilocks). Inside the risc0 guest,
//! `sha2` resolves to risc0's accelerated SHA-256 implementation via the
//! standard ecosystem patch.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod abi;
pub mod hash;
pub mod merkle;
#[cfg(feature = "host")]
pub mod merkle_store;
pub mod object;
pub mod tx;
pub mod value;

pub use hash::{Hash, ZERO_HASH, sha256, sha256_concat};
pub use object::Object;
pub use tx::{NULLIFIER_VERSION, StateRoot, Tx, compute_nullifier};
pub use value::Value;
