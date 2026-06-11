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

use axum::Json;
use wire_types::HealthResponse;

pub async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse::stamped(
        crate::RELEASE_TAG,
        crate::TARGET_TRIPLE,
    ))
}
