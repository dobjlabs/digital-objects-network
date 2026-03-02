use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Default)]
struct AppState {
    posts: Mutex<Vec<PostDto>>,
    next_id: AtomicU64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProofClaimDto {
    name: String,
    validity: String,
    hash: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PostDto {
    id: String,
    title: String,
    peer: String,
    time: String,
    desc: String,
    proofs: Vec<ProofClaimDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MockStateDto {
    post_count: usize,
    supported_methods: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunMethodInput {
    method_name: String,
    args: Vec<String>,
    cpu_cost: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProofRunResult {
    success: bool,
    method_name: String,
    old_root: String,
    new_root: String,
    stage_messages: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyPostInput {
    post_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifyResult {
    post_id: String,
    status: String,
    checked_block: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatePostInput {
    title: String,
    desc: String,
    proof_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RespondPostInput {
    post_id: String,
    desc: String,
    proof_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachClaimInput {
    file_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AttachClaimResult {
    name: String,
    validity: String,
    hash: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenericActionResult {
    ok: bool,
    message: String,
}

fn new_id(state: &AppState) -> String {
    let id = state.next_id.fetch_add(1, Ordering::Relaxed);
    format!("post-{id}")
}

fn now_label() -> String {
    "mock-now".to_string()
}

fn fake_hash(seed: &str) -> String {
    let mut bytes = [0u8; 8];
    for (idx, b) in seed.bytes().enumerate() {
        bytes[idx % 8] = bytes[idx % 8].wrapping_add(b);
    }
    format!("0x{:02x}{:02x}...{:02x}{:02x}", bytes[0], bytes[1], bytes[6], bytes[7])
}

#[tauri::command]
fn get_mock_state(state: tauri::State<'_, AppState>) -> MockStateDto {
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
fn run_method(input: RunMethodInput) -> ProofRunResult {
    let seed = format!("{}-{}-{}", input.method_name, input.args.join(","), input.cpu_cost);
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
fn verify_post_proofs(input: VerifyPostInput) -> VerifyResult {
    VerifyResult {
        post_id: input.post_id,
        status: "verified".to_string(),
        checked_block: "18,442,731".to_string(),
    }
}

#[tauri::command]
fn create_post(
    state: tauri::State<'_, AppState>,
    input: CreatePostInput,
) -> Result<PostDto, String> {
    let id = new_id(&state);
    let post = PostDto {
        id,
        title: input.title,
        peer: "127.0.0.1".to_string(),
        time: now_label(),
        desc: input.desc,
        proofs: input
            .proof_names
            .iter()
            .map(|name| ProofClaimDto {
                name: name.clone(),
                validity: "live".to_string(),
                hash: fake_hash(name),
            })
            .collect(),
    };
    let mut posts = state
        .posts
        .lock()
        .map_err(|_| "failed to acquire post state lock".to_string())?;
    posts.push(post.clone());
    Ok(post)
}

#[tauri::command]
fn respond_post(
    state: tauri::State<'_, AppState>,
    input: RespondPostInput,
) -> Result<GenericActionResult, String> {
    let posts = state
        .posts
        .lock()
        .map_err(|_| "failed to acquire post state lock".to_string())?;
    let target_exists = posts.iter().any(|post| post.id == input.post_id);
    drop(posts);

    if !target_exists {
        return Err(format!("post {} not found", input.post_id));
    }

    Ok(GenericActionResult {
        ok: true,
        message: format!(
            "mock response accepted ({} proofs attached): {}",
            input.proof_names.len(),
            input.desc
        ),
    })
}

#[tauri::command]
fn attach_claim(input: AttachClaimInput) -> AttachClaimResult {
    let name = input.file_name.trim_end_matches(".dobj").to_string();
    AttachClaimResult {
        name: name.clone(),
        validity: "live".to_string(),
        hash: fake_hash(&name),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_mock_state,
            run_method,
            verify_post_proofs,
            create_post,
            respond_post,
            attach_claim
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
