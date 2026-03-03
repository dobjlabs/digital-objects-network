use crate::types::{CreateDobjInput, CreateDobjResult};

fn fake_hash(seed: &str) -> String {
    let mut bytes = [0u8; 8];
    for (idx, b) in seed.bytes().enumerate() {
        bytes[idx % 8] = bytes[idx % 8].wrapping_add(b);
    }
    format!(
        "0x{:02x}{:02x}...{:02x}{:02x}",
        bytes[0], bytes[1], bytes[6], bytes[7]
    )
}

#[tauri::command]
pub fn create_dobj(input: CreateDobjInput) -> CreateDobjResult {
    let seed = format!("{}:{}", input.dobj_id, input.input_files.join(","));
    let old_root = fake_hash(&format!("{seed}:old"));
    let new_root = fake_hash(&format!("{seed}:new"));

    CreateDobjResult {
        ok: true,
        old_root,
        new_root,
        output_file: format!("{}.dobj", input.dobj_id),
    }
}
