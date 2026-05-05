use axum::{Json, extract::State};
use driver::DriverSettings;

use crate::error::ApiResult;
use crate::state::AppState;

pub async fn get_settings(State(state): State<AppState>) -> ApiResult<Json<DriverSettings>> {
    let settings = state.driver.load_settings()?;
    Ok(Json(settings))
}

pub async fn put_settings(
    State(state): State<AppState>,
    Json(input): Json<DriverSettings>,
) -> ApiResult<Json<DriverSettings>> {
    let saved = state.driver.save_settings(&input)?;
    Ok(Json(saved))
}
