//! Stamps the release tag and target triple into the binary so `/healthz`
//! reports the release it shipped in. The stamping logic is shared with the
//! CLI and the other services via `include!` of `../../build-stamp.rs`.

include!("../../build-stamp.rs");

fn main() {
    stamp_build_version();
    bundle_ui();
}

fn bundle_ui() {
    use std::{env, fs, path::Path};

    let output = Path::new(&env::var("OUT_DIR").unwrap()).join("bundled_ui.rs");
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
            files.sort();
            for path in files.into_iter().filter(|path| {
                !matches!(
                    path.file_name().and_then(|name| name.to_str()),
                    Some("AGENTS.md" | "CLAUDE.md" | "BITCRAFT_SPECIFIC_RULES.md")
                )
            }) {
                let name = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .replace('\\', "/");
                source.push_str(&format!(
                    "    ({name:?}, include_bytes!({:?})),\n",
                    path.to_str().unwrap()
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
