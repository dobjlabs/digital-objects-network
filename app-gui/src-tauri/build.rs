use std::{
    env, fs,
    path::{Path, PathBuf},
};

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
            if candidate.join("libscip.9.2.dylib").exists()
                || candidate.join("libscip.dylib").exists()
            {
                return Some(candidate);
            }
        }
    }

    None
}

fn copy_scip_dylibs(lib_dir: &Path, bundle_libs_dir: &Path) {
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

        let is_scip_dylib = file_name.starts_with("libscip") && file_name.ends_with(".dylib");
        if !is_scip_dylib {
            continue;
        }

        let dest = bundle_libs_dir.join(file_name);
        let _ = fs::copy(&path, dest);
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=DEP_SCIP_LIBDIR");

    #[cfg(target_os = "macos")]
    {
        let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
        let bundle_libs_dir = manifest_dir.join("libs");
        let _ = fs::create_dir_all(&bundle_libs_dir);

        // Resolve libscip from inside the app bundle at runtime.
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Resources/libs");

        // Keep common fallback paths for local dev environments.
        println!("cargo:rustc-link-arg=-Wl,-rpath,/opt/homebrew/opt/scipopt/lib");
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/local/opt/scipopt/lib");

        if let Ok(scip_lib_dir) = env::var("DEP_SCIP_LIBDIR") {
            copy_scip_dylibs(Path::new(&scip_lib_dir), &bundle_libs_dir);
            println!(
                "cargo:warning=Bundling SCIP dylibs from DEP_SCIP_LIBDIR={}",
                scip_lib_dir
            );
        } else if let Some(scip_lib_dir) = find_scip_lib_dir_from_target(&manifest_dir) {
            copy_scip_dylibs(&scip_lib_dir, &bundle_libs_dir);
            println!(
                "cargo:warning=Bundling SCIP dylibs from {}",
                scip_lib_dir.display()
            );
        } else {
            println!("cargo:warning=Could not locate SCIP dylibs; libs directory left empty");
        }
    }

    tauri_build::build()
}
