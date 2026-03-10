pub mod macros;
pub mod mockintro;

use plonky2::field::types::Field;
use pod2::middleware::{RawValue, F};
use rand::{rngs::StdRng, RngCore, SeedableRng};
use std::array;

pub fn rand_raw_value() -> RawValue {
    let mut rng = StdRng::from_os_rng();
    RawValue(array::from_fn(|_| F::from_noncanonical_u64(rng.next_u64())))
}
