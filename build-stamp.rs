// Shared build-script logic pulled into each binary's `build.rs` with
// `include!("../../build-stamp.rs")` (the CLI, dobjd, and the relayer,
// synchronizer, and archiver services). Kept as an included file rather than a
// build-dependency crate to avoid the workspace + Cargo plumbing for ~10 lines;
// the crates aren't published, so the out-of-package `include!` path is fine.

/// Stamp the release tag and target triple into the binary as the env vars
/// `DOBJ_RELEASE_TAG` and `DOBJ_TARGET_TRIPLE` (read back via `env!`). The
/// release workflow sets `DOBJ_RELEASE_TAG`; local builds fall back to "dev".
fn stamp_build_version() {
    println!("cargo:rerun-if-env-changed=DOBJ_RELEASE_TAG");
    // This file lives outside the crate dir, so cargo's default package-change
    // detection wouldn't notice edits to it; track it explicitly.
    println!("cargo:rerun-if-changed=../../build-stamp.rs");
    let tag = std::env::var("DOBJ_RELEASE_TAG").unwrap_or_else(|_| "dev".to_string());
    // TARGET is set by cargo for every build script run; it is the triple
    // being compiled *for* (not the host), so cross-compiled binaries stamp
    // the platform they will run on.
    let target = std::env::var("TARGET").expect("cargo always sets TARGET");
    println!("cargo:rustc-env=DOBJ_RELEASE_TAG={tag}");
    println!("cargo:rustc-env=DOBJ_TARGET_TRIPLE={target}");
}
