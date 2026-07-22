pub mod macros;
pub mod mockintro;

use alloy_primitives::B256;
use plonky2::field::types::Field;
use pod2::middleware::{Hash, RawValue, EMPTY_HASH, F};
use rand::{rngs::StdRng, RngCore, SeedableRng};
use std::array;
use std::cell::RefCell;

thread_local! {
    /// When `Some`, `rand_raw_value` advances this RNG instead of
    /// pulling fresh entropy. Set via [`set_seed`] for reproducible
    /// runs (dev tools, integration tests).
    static SEEDED_RNG: RefCell<Option<StdRng>> = const { RefCell::new(None) };
}

/// Switch this thread's `rand_raw_value` to a deterministic RNG
/// seeded with `seed`. Subsequent calls draw from the same stream
/// until [`clear_seed`] is called.
pub fn set_seed(seed: u64) {
    SEEDED_RNG.with(|cell| *cell.borrow_mut() = Some(StdRng::seed_from_u64(seed)));
}

/// Restore the default OS-seeded behavior for this thread.
pub fn clear_seed() {
    SEEDED_RNG.with(|cell| *cell.borrow_mut() = None);
}

pub fn rand_raw_value() -> RawValue {
    SEEDED_RNG.with(|cell| match cell.borrow_mut().as_mut() {
        Some(rng) => RawValue(array::from_fn(|_| F::from_noncanonical_u64(rng.next_u64()))),
        None => {
            let mut rng = StdRng::from_os_rng();
            RawValue(array::from_fn(|_| F::from_noncanonical_u64(rng.next_u64())))
        }
    })
}

/// Lossy conversion of a 256 bit value to a pod2 Hash
pub fn b256_to_hash(v: B256) -> Hash {
    let mut limbs: [[u8; 8]; 4] = Default::default();
    for (i, b) in v.0.iter().enumerate() {
        limbs[i / 8][i % 8] = *b;
    }
    let mut hash = EMPTY_HASH;
    for (i, l) in limbs.into_iter().enumerate() {
        hash.0[i] = F::from_noncanonical_u64(u64::from_le_bytes(l));
    }
    hash
}
