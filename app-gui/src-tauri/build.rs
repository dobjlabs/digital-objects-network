use std::{fs, path::PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::path::Path;
#[cfg(target_os = "macos")]
use std::{collections::VecDeque, process::Command};

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
            let has_scip_lib =
                candidate.join("libscip.so.9.2").exists() || candidate.join("libscip.so").exists();

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
        let is_scip_shared_lib = file_name.starts_with("libscip") && file_name.ends_with(".dylib");

        #[cfg(target_os = "linux")]
        let is_scip_shared_lib = file_name.starts_with("libscip") && file_name.contains(".so");

        if !is_scip_shared_lib {
            continue;
        }

        let dest = bundle_libs_dir.join(file_name);
        let _ = copy_dylib(&path, &dest);
    }
}

#[cfg(unix)]
fn make_writable_and_executable(path: &Path) {
    if let Ok(metadata) = fs::metadata(path) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o755);
        let _ = fs::set_permissions(path, permissions);
    }
}

#[cfg(unix)]
fn copy_dylib(src: &Path, dest: &Path) -> std::io::Result<u64> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    if dest.exists() {
        make_writable_and_executable(dest);
        let _ = fs::remove_file(dest);
    }
    let bytes = fs::copy(src, dest)?;
    make_writable_and_executable(dest);
    Ok(bytes)
}

#[cfg(target_os = "macos")]
fn run_command(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

#[cfg(target_os = "macos")]
fn ad_hoc_sign(path: &Path) {
    let Some(path_str) = path.to_str() else {
        return;
    };
    let _ = Command::new("codesign")
        .args(["--force", "--sign", "-", "--timestamp=none", path_str])
        .status();
}

#[cfg(target_os = "macos")]
fn dylib_dependencies(path: &Path) -> Vec<String> {
    let Some(path_str) = path.to_str() else {
        return Vec::new();
    };
    let Some(output) = run_command("otool", &["-L", path_str]) else {
        return Vec::new();
    };

    output
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(" (").map(|(dep, _)| dep.trim().to_string()))
        .collect()
}

#[cfg(target_os = "macos")]
fn resign_mac_dylibs(bundle_libs_dir: &Path) {
    let Ok(entries) = fs::read_dir(bundle_libs_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext == "dylib")
        {
            ad_hoc_sign(&path);
        }
    }
}

#[cfg(target_os = "macos")]
fn is_non_system_dylib(path: &str) -> bool {
    (path.starts_with("/opt/homebrew/") || path.starts_with("/usr/local/"))
        && path.ends_with(".dylib")
}

#[cfg(target_os = "macos")]
fn rewrite_mac_dylib_ids(bundle_libs_dir: &Path) {
    let Ok(entries) = fs::read_dir(bundle_libs_dir) else {
        return;
    };

    let mut queue = VecDeque::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext == "dylib")
        {
            queue.push_back(path);
        }
    }

    while let Some(path) = queue.pop_front() {
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(path_str) = path.to_str() else {
            continue;
        };

        let _ = Command::new("install_name_tool")
            .args(["-id", &format!("@rpath/{file_name}"), path_str])
            .status();

        for dep in dylib_dependencies(&path) {
            if !is_non_system_dylib(&dep) {
                continue;
            }

            let dep_path = PathBuf::from(&dep);
            let Some(dep_name) = dep_path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let dep_dest = bundle_libs_dir.join(dep_name);
            if !dep_dest.exists() {
                let _ = copy_dylib(&dep_path, &dep_dest);
                queue.push_back(dep_dest.clone());
            } else {
                make_writable_and_executable(&dep_dest);
            }

            let _ = Command::new("install_name_tool")
                .args(["-change", &dep, &format!("@rpath/{dep_name}"), path_str])
                .status();
        }
    }
}

#[cfg(target_os = "macos")]
fn copy_mac_gcc_runtime_libs(bundle_libs_dir: &Path) {
    let gcc_roots = [
        PathBuf::from("/opt/homebrew/opt/gcc/lib/gcc/current"),
        PathBuf::from("/usr/local/opt/gcc/lib/gcc/current"),
    ];
    let runtime_names = [
        "libgfortran.5.dylib",
        "libquadmath.0.dylib",
        "libgcc_s.1.1.dylib",
    ];

    for root in gcc_roots {
        if !root.exists() {
            continue;
        }

        for name in runtime_names {
            let src = root.join(name);
            let dest = bundle_libs_dir.join(name);
            if src.exists() {
                let _ = copy_dylib(&src, &dest);
            }
        }
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=DEP_SCIP_LIBDIR");
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let bundle_libs_dir = manifest_dir.join("libs");
    let _ = fs::create_dir_all(&bundle_libs_dir);

    #[cfg(target_os = "macos")]
    {
        // Resolve libscip from the signed Frameworks directory inside the app bundle.
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");

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

        copy_mac_gcc_runtime_libs(&bundle_libs_dir);
        rewrite_mac_dylib_ids(&bundle_libs_dir);
        resign_mac_dylibs(&bundle_libs_dir);
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
