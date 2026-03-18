use std::{fs, path::PathBuf};

#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::path::Path;

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn find_scip_lib_dir_from_target(manifest_dir: &Path) -> Option<PathBuf> {
    let workspace_root = manifest_dir.parent()?.parent()?;
    let target_dir = workspace_root.join("target");

    for profile in ["release", "debug"] {
        let build_dir = target_dir.join(profile).join("build");
        let Ok(entries) = fs::read_dir(build_dir) else {
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
            let has_scip_lib = candidate.join("libscip.9.2.dylib").exists()
                || candidate.join("libscip.dylib").exists();

            #[cfg(target_os = "linux")]
            let has_scip_lib = candidate.join("libscip.so.9.2").exists()
                || candidate.join("libscip.so").exists();

            if has_scip_lib {
                return Some(candidate);
            }
        }
    }

    None
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn copy_scip_shared_libs(lib_dir: &Path, bundle_libs_dir: &Path) {
    if fs::create_dir_all(bundle_libs_dir).is_err() {
        return;
    }

    let Ok(entries) = fs::read_dir(lib_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        #[cfg(target_os = "macos")]
        let is_scip_shared_lib =
            file_name.starts_with("libscip") && file_name.ends_with(".dylib");

        #[cfg(target_os = "linux")]
        let is_scip_shared_lib = file_name.starts_with("libscip") && file_name.contains(".so");

        if !is_scip_shared_lib {
            continue;
        }

        let dest = bundle_libs_dir.join(file_name);
        let _ = fs::copy(&path, dest);
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=DEP_SCIP_LIBDIR");
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let bundle_libs_dir = manifest_dir.join("libs");
    let _ = fs::create_dir_all(&bundle_libs_dir);

    #[cfg(target_os = "macos")]
    {
        // Resolve libscip from inside the app bundle at runtime.
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Resources/libs");

        // Keep common fallback paths for local dev environments.
        println!("cargo:rustc-link-arg=-Wl,-rpath,/opt/homebrew/opt/scipopt/lib");
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/local/opt/scipopt/lib");

        if let Ok(scip_lib_dir) = std::env::var("DEP_SCIP_LIBDIR") {
            copy_scip_shared_libs(Path::new(&scip_lib_dir), &bundle_libs_dir);
            println!(
                "cargo:warning=Bundling SCIP dylibs from DEP_SCIP_LIBDIR={}",
                scip_lib_dir
            );
        } else if let Some(scip_lib_dir) = find_scip_lib_dir_from_target(&manifest_dir) {
            copy_scip_shared_libs(&scip_lib_dir, &bundle_libs_dir);
            println!(
                "cargo:warning=Bundling SCIP dylibs from {}",
                scip_lib_dir.display()
            );
        } else {
            println!("cargo:warning=Could not locate SCIP dylibs; libs directory left empty");
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Tauri resources on Linux live under ../lib/<exe_name>, so the bundled
        // SCIP shared objects are placed under ../lib/app-gui/libs.
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib/app-gui/libs");

        if let Ok(scip_lib_dir) = std::env::var("DEP_SCIP_LIBDIR") {
            copy_scip_shared_libs(Path::new(&scip_lib_dir), &bundle_libs_dir);
            println!(
                "cargo:warning=Bundling SCIP shared libs from DEP_SCIP_LIBDIR={}",
                scip_lib_dir
            );
        } else if let Some(scip_lib_dir) = find_scip_lib_dir_from_target(&manifest_dir) {
            copy_scip_shared_libs(&scip_lib_dir, &bundle_libs_dir);
            println!(
                "cargo:warning=Bundling SCIP shared libs from {}",
                scip_lib_dir.display()
            );
        } else {
            println!("cargo:warning=Could not locate SCIP shared libs; libs directory left empty");
        }
    }

    tauri_build::build()
}
