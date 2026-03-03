use crate::state::AppState;
use crate::types::{CreatePostInput, GenericActionResult, PostDto, ProofClaimDto, RespondPostInput};

use super::utils::{fake_hash, new_id, now_label};

#[tauri::command]
pub fn create_post(
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
pub fn respond_post(
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
