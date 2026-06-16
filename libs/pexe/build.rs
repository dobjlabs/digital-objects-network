//! Stamps the release tag and target triple into the binary so `pexe
//! --version` reports the release it shipped in, matching dobj and dobjd.
//! The stamping logic is shared via `include!` of `../../build-stamp.rs`.

include!("../../build-stamp.rs");

fn main() {
    stamp_build_version();
}
