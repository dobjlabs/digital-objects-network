use std::{error::Error, fs, path::PathBuf};

#[path = "../action_spec.rs"]
mod action_spec;
#[path = "../contract_codegen.rs"]
mod contract_codegen;

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output_path = manifest_dir.join("../src/shared/generated/contracts.ts");
    let contracts = contract_codegen::render_typescript_contracts();
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output_path, contracts)?;
    println!("generated {}", output_path.display());
    Ok(())
}
