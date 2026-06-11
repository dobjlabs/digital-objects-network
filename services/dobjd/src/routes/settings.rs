use axum::{Json, extract::State};
use wire_types::{DriverSettings, DriverSettingsPatch};

use crate::error::ApiResult;
use crate::state::AppState;

pub async fn get_settings(State(state): State<AppState>) -> ApiResult<Json<DriverSettings>> {
    let settings = state.driver.load_settings()?;
    Ok(Json(settings))
}

pub async fn put_settings(
    State(state): State<AppState>,
    Json(patch): Json<DriverSettingsPatch>,
) -> ApiResult<Json<DriverSettings>> {
    // Merge onto the current settings so an absent field keeps its value
    // rather than reverting to a type default -- in particular, a body that
    // omits `mcpEnabled` must not stop a running MCP server.
    let mut merged = state.driver.load_settings()?;
    patch.apply_to(&mut merged);
    // Reconcile the MCP server before persisting, so a failed enable (the
    // adjacent port is taken) is reported to the caller without recording
    // a setting that isn't in effect. If `apply` succeeds but the save below
    // then fails, the running server and the persisted setting diverge with
    // no rollback; save failures are rare and otherwise fatal, so we accept
    // it rather than add a compensating stop/restart.
    state.mcp.apply(merged.mcp_enabled).await?;
    let saved = state.driver.save_settings(&merged)?;
    Ok(Json(saved))
}
