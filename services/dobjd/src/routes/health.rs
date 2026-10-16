//! Liveness probe for `dobj start` / `dobj status`.
//!
//! Returning 200 from this endpoint is itself the signal: dobjd's HTTP
//! listener only binds *after* `Driver::open_default()` succeeds in main -
//! plugin catalog loaded, RocksDB opened, paths resolved. So a successful
//! response here means the daemon initialized cleanly. No further work is
//! needed in the handler.
//!
//! The body is `wire_types::HealthResponse`, the shared health shape across
//! dobjd and the relayer/synchronizer/archiver services: the `ok` liveness
//! flag plus the version/target stamp so clients can tell which build is
//! serving.
//! An optional instance_id echoes DOBJ_INSTANCE_ID for process supervisors.

use axum::Json;
use serde::Serialize;
use wire_types::HealthResponse;

#[derive(Serialize)]
pub struct DaemonHealth {
    #[serde(flatten)]
    health: HealthResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    instance_id: Option<String>,
}

pub async fn healthz() -> Json<DaemonHealth> {
    Json(DaemonHealth {
        health: HealthResponse::stamped(crate::RELEASE_TAG, crate::TARGET_TRIPLE),
        instance_id: std::env::var("DOBJ_INSTANCE_ID").ok(),
    })
}
