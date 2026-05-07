use axum::{
    Json,
    extract::{Multipart, Path, State},
};
use driver::{ObjectRecord, ObjectSelector, ObjectSummary, parse_object_record_bytes};
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

/// `GET /objects/{id}` — read a single object by its content-addressed id.
/// Routes a hex commitment (e.g. `0xabc...`) into `Driver::read_object`.
pub async fn inspect_object(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ObjectSummary>> {
    let driver = state.driver.clone();
    let summary =
        tokio::task::spawn_blocking(move || driver.read_object(&ObjectSelector::ObjectId(id)))
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
/// input, it must live in `~/.dobj/objects/` (the existing constraint from
/// `ObjectSelector::FileName`).
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
