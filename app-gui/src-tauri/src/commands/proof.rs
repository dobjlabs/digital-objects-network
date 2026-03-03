use crate::state::AppState;
use crate::types::{
    AttachClaimInput, AttachClaimResult, MockStateDto, ProofRunResult, RunMethodInput,
    VerifyPostInput, VerifyResult,
};

use super::utils::fake_hash;

#[tauri::command]
pub fn get_mock_state(state: tauri::State<'_, AppState>) -> MockStateDto {
    let post_count = state.posts.lock().map(|posts| posts.len()).unwrap_or(0);
    MockStateDto {
        post_count,
        supported_methods: vec![
            "extract".to_string(),
            "feed".to_string(),
            "transfer".to_string(),
            "mint".to_string(),
        ],
    }
}

#[tauri::command]
pub fn run_method(input: RunMethodInput) -> ProofRunResult {
    let seed = format!(
        "{}-{}-{}-{}",
        input.id,
        input.method_name,
        input.input_files.join(","),
        input.cpu_cost
    );
    let old_root = fake_hash(&format!("{seed}-old"));
    let new_root = fake_hash(&format!("{seed}-new"));
    ProofRunResult {
        success: true,
        method_name: input.method_name,
        old_root: old_root.clone(),
        new_root: new_root.clone(),
        stage_messages: vec![
            format!("Generating recursive proof for {}", old_root),
            "Nullifying old state root".to_string(),
            format!("Committing new state root {}", new_root),
        ],
    }
}

#[tauri::command]
pub fn verify_post_proofs(input: VerifyPostInput) -> VerifyResult {
    VerifyResult {
        post_id: input.post_id,
        status: "verified".to_string(),
        checked_block: "18,442,731".to_string(),
    }
}

#[tauri::command]
pub fn attach_claim(input: AttachClaimInput) -> AttachClaimResult {
    let name = input.file_name.trim_end_matches(".dobj").to_string();
    AttachClaimResult {
        name: name.clone(),
        validity: "live".to_string(),
        hash: fake_hash(&name),
    }
}
