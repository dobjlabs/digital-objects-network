use std::path::PathBuf;

use axum::{
    Json,
    extract::{Multipart, Path, State},
};
use driver::{ObjectRecord, ObjectSummary, parse_object_record_bytes};
use serde::Serialize;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Cap for `/objects/parse` multipart uploads. A `.dobj` file is small
/// JSON wrapping a pod proof — a few hundred KiB at most. 16 MiB is two
/// orders of magnitude over realistic input and protects the daemon from
/// a hostile/buggy frontend uploading multi-GB blobs.
pub const MAX_DOBJ_UPLOAD_BYTES: usize = 16 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectsDirResponse {
    pub path: String,
}

pub async fn get_objects_dir(State(state): State<AppState>) -> ApiResult<Json<ObjectsDirResponse>> {
    let path = state.driver.paths().objects_dir.clone();
    Ok(Json(ObjectsDirResponse {
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

/// Parse an uploaded `.dobj` file in-memory and return the resulting
/// [`ObjectRecord`]. Mirrors `read_dobj_file` but works on a multipart upload
/// rather than an absolute path — the browser has no notion of the local
/// filesystem, so the frontend reads bytes from a drop and POSTs them here.
///
/// No disk write happens on the server. To actually use a file as action
/// input, it must live in `~/.dobj/objects/` (the constraint
/// `ExecuteActionInput.input_objects` enforces — basenames only).
pub async fn parse_object(mut multipart: Multipart) -> ApiResult<Json<ObjectRecord>> {
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| ApiError::bad_request(format!("multipart read failed: {err}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|err| ApiError::bad_request(format!("failed to read upload: {err}")))?;
        let record = parse_object_record_bytes(&bytes)?;
        return Ok(Json(record));
    }
    Err(ApiError::bad_request(
        "missing 'file' field in multipart upload",
    ))
}
