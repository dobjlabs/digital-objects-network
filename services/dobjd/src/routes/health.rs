//! Liveness probe for `dobj start` / `dobj status`.
//!
//! Returning 200 from this endpoint is itself the signal: dobjd's HTTP
//! listener only binds *after* `Driver::open_default()` succeeds in main -
//! plugin catalog loaded, RocksDB opened, paths resolved. So a successful
//! response here means the daemon initialized cleanly. No further work is
//! needed in the handler.
//!
//! The body is `wire_types::HealthResponse`, a superset of the
//! synchronizer/relayer health shape: the same `ok` field, plus the
//! version/target stamp so clients can tell which build is serving.

use axum::Json;
use wire_types::HealthResponse;

pub async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        version: Some(crate::RELEASE_TAG.to_string()),
        target: Some(crate::TARGET_TRIPLE.to_string()),
    })
}
