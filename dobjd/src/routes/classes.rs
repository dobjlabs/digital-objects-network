//! Routes for inspecting the action catalog's class definitions.
//!
//! `list_inventory` (under `/inventory`) folds objects + actions together
//! for the gameplay-style UIs, but classes are independently useful for
//! introspection — predicate sources, related actions, live counts. These
//! handlers expose `Driver::list_classes` and `Driver::get_class` directly
//! for the CLI, the website, and any other client that wants the full
//! catalog.

use axum::{
    Json,
    extract::{Path, State},
};
use driver::ClassSummary;

use crate::error::ApiResult;
use crate::state::AppState;

/// `GET /classes` — full list of every class the loaded plugins declare.
pub async fn list_classes(State(state): State<AppState>) -> ApiResult<Json<Vec<ClassSummary>>> {
    let driver = state.driver.clone();
    let classes = tokio::task::spawn_blocking(move || driver.list_classes())
        .await
        .map_err(|err| anyhow::anyhow!("list_classes task panicked: {err}"))??;
    Ok(Json(classes))
}

/// `GET /classes/{name}` — one class detail with predicate source.
pub async fn inspect_class(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<ClassSummary>> {
    let driver = state.driver.clone();
    let class = tokio::task::spawn_blocking(move || driver.get_class(&name))
        .await
        .map_err(|err| anyhow::anyhow!("get_class task panicked: {err}"))??;
    Ok(Json(class))
}
