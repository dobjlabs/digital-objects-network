//! Stamps the release tag and target triple into the binary so `/healthz`
//! reports the release it shipped in. The stamping logic is shared with the
//! CLI and the other services via `include!` of `../../build-stamp.rs`.

include!("../../build-stamp.rs");

fn main() {
    stamp_build_version();
    bundle_ui();
}

fn bundle_ui() {
    use sha2::{Digest, Sha256};
    use std::{env, fs, path::Path};

    let output_directory = env::var("OUT_DIR").unwrap();
    let output_directory = Path::new(&output_directory);
    let output = output_directory.join("bundled_ui.rs");
    let excluded: Vec<_> = include_str!("ui-excluded-files.txt").lines().collect();
    println!("cargo:rerun-if-changed=ui-excluded-files.txt");
    let mut source = String::new();
    for (feature, constant, directory) in [
        (
            "CARGO_FEATURE_BUNDLED_UI",
            "BUNDLED_ASSETS",
            "../../interfaces/gui/dist",
        ),
        (
            "CARGO_FEATURE_BUNDLED_BITCRAFT_UI",
            "BITCRAFT_ASSETS",
            "../../apps/bitcraft/ui",
        ),
    ] {
        source.push_str(&format!("pub const {constant}: &[(&str, &[u8])] = &[\n"));
        if env::var_os(feature).is_some() {
            let root = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap())
                .join(directory)
                .canonicalize()
                .unwrap_or_else(|_| panic!("{feature} requires assets in {directory}"));
            assert!(
                root.join("index.html").is_file(),
                "UI build has no index.html"
            );
            println!("cargo:rerun-if-changed={}", root.display());
            let mut files = Vec::new();
            collect_assets(&root, &mut files);
            let mut assets: Vec<_> = files
                .into_iter()
                .filter(|path| !excluded.contains(&path.file_name().unwrap().to_str().unwrap()))
                .map(|path| {
                    let name = path
                        .strip_prefix(&root)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .replace('\\', "/");
                    (name, path)
                })
                .collect();
            assets.sort_by(|left, right| left.0.cmp(&right.0));
            let mut fingerprint = Sha256::new();
            fingerprint.update(b"dobjd-ui-v1\0");
            for (name, path) in &assets {
                fingerprint.update((name.len() as u64).to_le_bytes());
                fingerprint.update(name.as_bytes());
                fingerprint.update(Sha256::digest(fs::read(path).unwrap()));
            }
            let marker = format!("dobjd-{constant}-sha256-v1:{:x}", fingerprint.finalize());
            let mut index = fs::read(root.join("index.html")).unwrap();
            index.extend_from_slice(format!("\n<!-- {marker} -->\n").as_bytes());
            let bundled_index = output_directory.join(format!("{constant}_index.html"));
            fs::write(&bundled_index, index).unwrap();
            for (name, path) in assets {
                let embedded = if name == "index.html" {
                    &bundled_index
                } else {
                    &path
                };
                source.push_str(&format!(
                    "    ({name:?}, include_bytes!({:?})),\n",
                    embedded.to_str().unwrap()
                ));
            }
        }
        source.push_str("];\n");
    }
    fs::write(output, source).unwrap();
}

fn collect_assets(root: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        assert!(!kind.is_symlink(), "UI build must not contain symlinks");
        if kind.is_dir() {
            collect_assets(&entry.path(), files);
        } else if kind.is_file() {
            files.push(entry.path());
        }
    }
}
