//! Bake an RPATH into the `dobjd` binary so it can find `libscip` at runtime
//! without setting `DYLD_LIBRARY_PATH` / `LD_LIBRARY_PATH`.
//!
//! In the release tarball produced by `.github/workflows/release-cli.yml`,
//! `libscip*` lives next to the `dobjd` binary, so we point at:
//!
//! - macOS: `@loader_path` — the directory of the binary being loaded
//! - Linux: `$ORIGIN`     — same idea, ELF spelling
//!
//! For local dev (`cargo run -p dobjd`), cargo injects the build-output
//! libs path into `DYLD_LIBRARY_PATH` so the bare RPATH isn't needed; we
//! still keep two extra macOS fallbacks pointing at common Homebrew SCIP
//! installs, mirroring the desktop app's `build.rs`.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(target_os = "macos")]
    {
        // The release tarball lays libscip next to dobjd, so @loader_path
        // resolves to that directory.
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");

        // Fallbacks for users who installed SCIP via Homebrew and run a
        // locally-built dobjd outside the release-tarball layout.
        println!("cargo:rustc-link-arg=-Wl,-rpath,/opt/homebrew/opt/scipopt/lib");
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/local/opt/scipopt/lib");
    }

    #[cfg(target_os = "linux")]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    }
}
