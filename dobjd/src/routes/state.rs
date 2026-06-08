use axum::{Json, extract::State};

use crate::error::ApiResult;
use crate::state::AppState;

pub async fn get_state_root(State(state): State<AppState>) -> ApiResult<Json<String>> {
    let driver = state.driver.clone();
    let root = tokio::task::spawn_blocking(move || driver.get_state_root())
        .await
        .map_err(|err| anyhow::anyhow!("state root task panicked: {err}"))??;
    Ok(Json(common::encode_hash_hex(&root)))
}
