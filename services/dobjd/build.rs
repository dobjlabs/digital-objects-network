//! Stamps the release tag and target triple into the binary so `/healthz`
//! reports the release it shipped in. The stamping logic is shared with
//! `cli/build.rs` via `include!` of `../../build-stamp.rs`.

include!("../../build-stamp.rs");

fn main() {
    stamp_build_version();
}
