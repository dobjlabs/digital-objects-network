use axum::{Json, extract::Multipart, extract::State};
use driver::{ObjectRecord, parse_object_record_bytes};
use serde::Serialize;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

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
