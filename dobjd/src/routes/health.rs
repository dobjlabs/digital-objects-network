//! Liveness probe for `dobj start` / `dobj status`.
//!
//! Lives separately from `/objects/dir` because it carries semantics — it
//! signals "the catalog loaded and the driver is operable" rather than just
//! "the HTTP listener bound." A daemon whose plugin catalog failed or
//! whose RocksDB is wedged should fail this probe so `wait_until_ready`
//! reports the real problem instead of spinning for 60s.
//!
//! The check intentionally avoids touching the network (no synchronizer
//! round-trip) because a slow/unreachable synchronizer is a separate
//! failure mode — the daemon itself is still up and partially useful, and
//! we don't want a transient sync hiccup to flip the health state.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Serialize;

use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthOk {
    pub status: &'static str,
    pub action_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthDown {
    pub status: &'static str,
    pub error: String,
}

/// `GET /healthz` — 200 when the driver can list its action catalog, 503
/// otherwise. Network-free: never blocks on the synchronizer.
pub async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let driver = state.driver.clone();
    let result = tokio::task::spawn_blocking(move || driver.list_actions(None)).await;

    match result {
        Ok(Ok(actions)) => (
            StatusCode::OK,
            Json(
                serde_json::to_value(HealthOk {
                    status: "ok",
                    action_count: actions.len(),
                })
                .unwrap(),
            ),
        ),
        Ok(Err(err)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(
                serde_json::to_value(HealthDown {
                    status: "degraded",
                    error: err.to_string(),
                })
                .unwrap(),
            ),
        ),
        Err(err) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(
                serde_json::to_value(HealthDown {
                    status: "down",
                    error: format!("healthz task panicked: {err}"),
                })
                .unwrap(),
            ),
        ),
    }
}
