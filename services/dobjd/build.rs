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
    let mut source = "pub const BUNDLED_ASSETS: &[(&str, &[u8])] = &[\n".to_string();
    if env::var_os("CARGO_FEATURE_BUNDLED_UI").is_some() {
        let root = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("../../interfaces/gui/dist")
            .canonicalize()
            .expect("bundled-ui requires `pnpm build` in interfaces/gui first");
        assert!(
            root.join("index.html").is_file(),
            "UI build has no index.html"
        );
        println!("cargo:rerun-if-changed={}", root.display());
        let mut files = Vec::new();
        collect_assets(&root, &mut files);
        files.sort();
        for path in files {
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
