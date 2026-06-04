//! Stamps the release tag and target triple into the binary so `/healthz`
//! reports the release it shipped in. The release workflow sets
//! DOBJ_RELEASE_TAG to the tag being built; local builds fall back to "dev".

fn main() {
    println!("cargo:rerun-if-env-changed=DOBJ_RELEASE_TAG");
    let tag = std::env::var("DOBJ_RELEASE_TAG").unwrap_or_else(|_| "dev".to_string());
    // TARGET is set by cargo for every build script run; it is the triple
    // being compiled *for* (not the host), so cross-compiled binaries stamp
    // the platform they will run on.
    let target = std::env::var("TARGET").expect("cargo always sets TARGET");
    println!("cargo:rustc-env=DOBJ_RELEASE_TAG={tag}");
    println!("cargo:rustc-env=DOBJ_TARGET_TRIPLE={target}");
}
