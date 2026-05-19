use std::{fs, path::PathBuf};

fn find_scip_lib_dir_from_target() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").ok()?);
    let workspace_root = manifest_dir.parent()?;
    let target_dir = workspace_root.join("target");

    for profile in ["release", "debug"] {
        let build_dir = target_dir.join(profile).join("build");
        let Ok(entries) = fs::read_dir(&build_dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            if !file_name.starts_with("scip-sys-") {
                continue;
            }

            let candidate = entry.path().join("out/scip_install/lib");
            #[cfg(target_os = "macos")]
            let has_lib = candidate.join("libscip.9.2.dylib").exists()
                || candidate.join("libscip.dylib").exists();
            #[cfg(target_os = "linux")]
            let has_lib =
                candidate.join("libscip.so.9.2").exists() || candidate.join("libscip.so").exists();

            if has_lib {
                return Some(candidate);
            }
        }
    }

    None
}

fn main() {
    println!("cargo:rerun-if-env-changed=DEP_SCIP_LIBDIR");

    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/opt/homebrew/opt/scipopt/lib");
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/local/opt/scipopt/lib");
    }
    #[cfg(target_os = "linux")]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/x86_64-linux-gnu");
    }

    let scip_lib_dir = std::env::var("DEP_SCIP_LIBDIR")
        .ok()
        .map(PathBuf::from)
        .or_else(find_scip_lib_dir_from_target);

    if let Some(dir) = scip_lib_dir {
        if let Some(s) = dir.to_str() {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{s}");
            println!("cargo:warning=Adding SCIP rpath: {s}");
        }
    } else {
        println!(
            "cargo:warning=Could not locate SCIP lib dir; binary may fail to load libscip at runtime"
        );
    }
}
