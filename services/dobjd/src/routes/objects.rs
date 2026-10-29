use std::path::PathBuf;

use axum::{
    Json,
    extract::{Path, State},
    http::header,
    response::IntoResponse,
};
use driver::ObjectQuery;
use wire_types::{ImportObjectRequest, ObjectSummary, ObjectsDirInfo, RenameObjectRequest};

use crate::error::ApiResult;
use crate::events::Event;
use crate::state::AppState;

pub async fn get_objects_dir(State(state): State<AppState>) -> ApiResult<Json<ObjectsDirInfo>> {
    let path = state.services.driver.paths().objects_dir.clone();
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
    let services = state.services.clone();
    let summary = tokio::task::spawn_blocking(move || services.object_summary(&file_name))
        .await
        .map_err(|err| anyhow::anyhow!("inspect_object task panicked: {err}"))??;
    Ok(Json(summary))
}

/// `GET /objects/{file_name}/raw` -- the object file's bytes verbatim, served
/// as JSON. Resolves `file_name` the same way as `GET /objects/{file_name}`.
pub async fn read_object_raw(
    State(state): State<AppState>,
    Path(file_name): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let driver = state.services.driver.clone();
    let contents =
        tokio::task::spawn_blocking(move || driver.read_object_raw(&PathBuf::from(&file_name)))
            .await
            .map_err(|err| anyhow::anyhow!("read_object_raw task panicked: {err}"))??;
    Ok(([(header::CONTENT_TYPE, "application/json")], contents))
}

/// `POST /objects/{file_name}/discard_unsafe` -- move an unspent object's
/// file into `objects/.discarded/` directly, whatever its status (live,
/// pending, or unknown) and even while a run holds it, returning its summary
/// from before the move. Spent objects are not found (404).
///
/// This is an escape hatch, not an object-state transition: none of the
/// guarantees dobjd otherwise keeps about object state are promised. Nothing
/// is checked against the chain or reconciled, a live or soon-to-land object
/// is dropped from the store all the same, a run holding the object is not
/// told, and discarded files are never listed, synced, or used as action
/// inputs again unless re-imported.
pub async fn discard_object_unsafe(
    State(state): State<AppState>,
    Path(file_name): Path<String>,
) -> ApiResult<Json<ObjectSummary>> {
    let services = state.services.clone();
    let summary = tokio::task::spawn_blocking(move || {
        let summary = services
            .driver
            .discard_object_unsafe(&PathBuf::from(&file_name))?;
        services.events.publish(Event::ObjectRemoved {
            file_name: summary.file_name.clone(),
        });
        anyhow::Ok(summary)
    })
    .await
    .map_err(|err| anyhow::anyhow!("discard_object_unsafe task panicked: {err}"))??;
    Ok(Json(summary))
}

/// `POST /objects/{file_name}/rename` -- give an unspent object's file a new
/// name. Returns its summary under the new name. 400 for an unsafe name, 409
/// when the name is taken or the object is held, spent or still being
/// created by an open transaction.
pub async fn rename_object(
    State(state): State<AppState>,
    Path(file_name): Path<String>,
    Json(req): Json<RenameObjectRequest>,
) -> ApiResult<Json<ObjectSummary>> {
    let services = state.services.clone();
    let summary = tokio::task::spawn_blocking(move || {
        let summary = services.rename_object(&file_name, &req.file_name)?;
        anyhow::Ok(summary)
    })
    .await
    .map_err(|err| anyhow::anyhow!("rename_object task panicked: {err}"))??;
    Ok(Json(summary))
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
    let services = state.services.clone();
    let summary = tokio::task::spawn_blocking(move || {
        let summary = services
            .driver
            .import_object(&req.dobj, req.file_name.as_deref())?;
        services.events.publish(Event::Object(summary.clone()));
        anyhow::Ok(summary)
    })
    .await
    .map_err(|err| anyhow::anyhow!("import_object task panicked: {err}"))??;
    Ok(Json(summary))
}

/// `GET /objects` -- local objects synced against the chain, each already
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
    let services = state.services.clone();
    let objects =
        tokio::task::spawn_blocking(move || services.settler.sync_objects(query.as_ref()))
            .await
            .map_err(|err| anyhow::anyhow!("load_objects task panicked: {err}"))??;

    Ok(Json(objects))
}
