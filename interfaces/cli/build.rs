//! Stamps the release tag and target triple into the binary so `dobj
//! --version` reports the release it shipped in. The stamping logic is shared
//! with `dobjd/build.rs` via `include!` of `../../build-stamp.rs`.

include!("../../build-stamp.rs");

fn main() {
    stamp_build_version();
}
