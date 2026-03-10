use std::{error::Error, fs, path::PathBuf};

#[path = "../action_spec.rs"]
mod action_spec;
#[path = "../id_codegen.rs"]
mod id_codegen;

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_path = manifest_dir.join("../src/shared/generated/ids.ts");
    let ids = id_codegen::render_typescript_ids();
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output_path, ids)?;
    println!("generated {}", output_path.display());
    Ok(())
}
