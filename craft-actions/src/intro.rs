//! Intro-style sub-protocol verification.
//!
//! Replaces the pod2-era recursive intro pods (`vdfpod`, `lt_eq_u256_pod`)
//! with plain in-guest checks:
//!
//! - **VDF** = SHA-256 chain of length `iters`. The guest re-runs the chain
//!   to verify; cheap with risc0's SHA accelerator (~1k cycles per round).
//! - **LtEqU256** = byte-wise big-endian comparison.
//!
//! See [`txlib_core::abi::IntroWitness`] for the witness shape.

use txlib_core::Hash;
use txlib_core::abi::IntroWitness;
use txlib_core::hash::sha256;

/// Re-run a SHA-256 chain to verify a VDF witness. Panics on mismatch.
pub fn verify_vdf_chain(iters: u32, input: Hash, expected_output: Hash) {
    let mut current = input;
    for _ in 0..iters {
        current = sha256(current.as_bytes());
    }
    assert_eq!(current, expected_output, "VDF chain output mismatch");
}

/// Lex-compare two 32-byte big-endian values.
pub fn check_le_u256(lhs: &[u8; 32], rhs: &[u8; 32]) {
    assert!(lhs <= rhs, "u256 comparison failed: lhs > rhs");
}

/// Convenience: build a 32-byte big-endian value with `n` in the top 8 bytes
/// and zeros below. Matches the original SDK's `top_limb_u256(n)` semantics
/// (used as a difficulty target).
pub fn top_limb_u256(n: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[..8].copy_from_slice(&n.to_be_bytes());
    out
}

/// Resolve the first VDF witness by `iters` from the action's witness list,
/// returning `(input, output)`. Panics if no matching witness is found.
pub fn pull_vdf_witness(witnesses: &[IntroWitness], expected_iters: u32) -> (Hash, Hash) {
    for w in witnesses {
        if let IntroWitness::Vdf {
            iters,
            input,
            output,
        } = w
        {
            if *iters == expected_iters {
                return (*input, *output);
            }
        }
    }
    panic!("missing Vdf witness with iters={expected_iters}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vdf_chain_roundtrip() {
        let input = sha256(b"start");
        let mut expected = input;
        for _ in 0..5 {
            expected = sha256(expected.as_bytes());
        }
        verify_vdf_chain(5, input, expected); // doesn't panic
    }

    #[test]
    #[should_panic(expected = "VDF chain output mismatch")]
    fn vdf_chain_rejects_wrong_output() {
        let input = sha256(b"start");
        verify_vdf_chain(5, input, sha256(b"wrong"));
    }

    #[test]
    fn le_u256_passes_when_le() {
        check_le_u256(&[0u8; 32], &[1u8; 32]);
        check_le_u256(&[5u8; 32], &[5u8; 32]); // equal
    }

    #[test]
    #[should_panic(expected = "u256 comparison failed")]
    fn le_u256_panics_when_gt() {
        check_le_u256(&[2u8; 32], &[1u8; 32]);
    }

    #[test]
    fn top_limb_u256_layout() {
        let v = top_limb_u256(0x1234_5678_9abc_def0);
        assert_eq!(&v[..8], &0x1234_5678_9abc_def0u64.to_be_bytes());
        assert_eq!(&v[8..], &[0u8; 24]);
    }

    #[test]
    fn pull_vdf_finds_matching_iters() {
        let w = alloc::vec![
            IntroWitness::LtEqU256 {
                lhs: [0; 32],
                rhs: [0; 32],
            },
            IntroWitness::Vdf {
                iters: 3,
                input: sha256(b"a"),
                output: sha256(b"b"),
            },
        ];
        let (input, output) = pull_vdf_witness(&w, 3);
        assert_eq!(input, sha256(b"a"));
        assert_eq!(output, sha256(b"b"));
    }

    #[test]
    #[should_panic(expected = "missing Vdf witness with iters=10")]
    fn pull_vdf_panics_when_missing() {
        let w = alloc::vec![IntroWitness::Vdf {
            iters: 3,
            input: Hash::default(),
            output: Hash::default(),
        }];
        pull_vdf_witness(&w, 10);
    }
}
