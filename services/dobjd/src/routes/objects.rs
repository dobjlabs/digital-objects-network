use std::path::PathBuf;

use axum::{
    Json,
    extract::{Path, State},
    http::header,
    response::IntoResponse,
};
use driver::ObjectQuery;
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
    let mut summary =
        tokio::task::spawn_blocking(move || driver.read_object(&PathBuf::from(&file_name)))
            .await
            .map_err(|err| anyhow::anyhow!("inspect_object task panicked: {err}"))??;
    state
        .runs
        .fill_held_by_run_ids(std::slice::from_mut(&mut summary));
    Ok(Json(summary))
}

/// `GET /objects/{file_name}/raw` -- the object file's bytes verbatim, served
/// as JSON. Resolves `file_name` the same way as `GET /objects/{file_name}`.
pub async fn read_object_raw(
    State(state): State<AppState>,
    Path(file_name): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let driver = state.driver.clone();
    let contents =
        tokio::task::spawn_blocking(move || driver.read_object_raw(&PathBuf::from(&file_name)))
            .await
            .map_err(|err| anyhow::anyhow!("read_object_raw task panicked: {err}"))??;
    Ok(([(header::CONTENT_TYPE, "application/json")], contents))
}

/// `POST /objects/import` — adopt an external `.dobj` (one not produced by
/// this driver, e.g. from outside `~/.dobj/`) into the local object store. Body is `{ "dobj": "<json>" }`;
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

/// `GET /objects` — local objects synced against the chain, each already
/// carrying its class's emoji/description (the driver folds that in). The
/// action catalog comes from `GET /actions` separately, so clients can
/// fetch the two in parallel.
pub async fn load_objects(State(state): State<AppState>) -> ApiResult<Json<Vec<ObjectSummary>>> {
    synced_objects(state, None).await
}

/// `GET /objects/unspent` -- like `GET /objects`, minus nullified (spent)
/// objects.
pub async fn load_unspent_objects(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<ObjectSummary>>> {
    let query = ObjectQuery {
        exclude_nullified: true,
        ..ObjectQuery::default()
    };
    synced_objects(state, Some(query)).await
}

async fn synced_objects(
    state: AppState,
    query: Option<ObjectQuery>,
) -> ApiResult<Json<Vec<ObjectSummary>>> {
    let driver = state.driver.clone();
    let mut objects = tokio::task::spawn_blocking(move || {
        driver.sync_objects(query.as_ref()).unwrap_or_else(|err| {
            tracing::warn!("sync_objects failed, falling back to local: {err:#}");
            driver.list_objects(query.as_ref()).unwrap_or_default()
        })
    })
    .await
    .map_err(|err| anyhow::anyhow!("load_objects task panicked: {err}"))?;
    state.runs.fill_held_by_run_ids(&mut objects);

    Ok(Json(objects))
}
