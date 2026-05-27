use std::path::PathBuf;

use axum::{
    Json,
    extract::{Path, State},
};
use wire_types::{ImportObjectRequest, ObjectSummary, ObjectsDirInfo};

use crate::error::ApiResult;
use crate::state::AppState;

pub async fn get_objects_dir(State(state): State<AppState>) -> ApiResult<Json<ObjectsDirInfo>> {
    let path = state.driver.paths().objects_dir.clone();
    Ok(Json(ObjectsDirInfo {
        path: path.to_string_lossy().to_string(),
    }))
}

/// `GET /objects/{file_name}` — read a single object's summary. The path
/// segment is treated as a basename within `~/.dobj/objects/` (or
/// `.nullified/` for spent objects). The driver normalizes the input
/// via `Path::file_name`, so traversal attempts (`..`) resolve to no
/// match and surface as 404, never as an arbitrary read.
pub async fn inspect_object(
    State(state): State<AppState>,
    Path(file_name): Path<String>,
) -> ApiResult<Json<ObjectSummary>> {
    let driver = state.driver.clone();
    let summary =
        tokio::task::spawn_blocking(move || driver.read_object(&PathBuf::from(&file_name)))
            .await
            .map_err(|err| anyhow::anyhow!("inspect_object task panicked: {err}"))??;
    Ok(Json(summary))
}

/// `POST /objects/import` — adopt a `.dobj` received out-of-band (e.g. a
/// trade attachment) into local inventory. Body is `{ "dobj": "<json>" }`;
/// the driver validates class identity + on-chain grounding and files it
/// under a canonical name. Returns the filed object's summary. 409 if the
/// object is already held or already spent on-chain.
pub async fn import_object(
    State(state): State<AppState>,
    Json(req): Json<ImportObjectRequest>,
) -> ApiResult<Json<ObjectSummary>> {
    let driver = state.driver.clone();
    let summary = tokio::task::spawn_blocking(move || driver.import_object(&req.dobj))
        .await
        .map_err(|err| anyhow::anyhow!("import_object task panicked: {err}"))??;
    Ok(Json(summary))
}
