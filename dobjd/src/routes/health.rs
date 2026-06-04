//! Liveness probe for `dobj start` / `dobj status`.
//!
//! Returning 200 from this endpoint is itself the signal: dobjd's HTTP
//! listener only binds *after* `Driver::open_default()` succeeds in main —
//! plugin catalog loaded, RocksDB opened, paths resolved. So a successful
//! response here means the daemon initialized cleanly. No further work is
//! needed in the handler.
//!
//! A superset of the synchronizer/relayer `/healthz` wire shape: same `ok`
//! field, so tooling that probes all three keeps one parser, plus the
//! version/target stamp so clients can tell which build is serving.

use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct HealthResponse {
    pub ok: bool,
    /// Release tag this binary was built from ("dev" outside a release).
    pub version: &'static str,
    /// Target triple this binary was built for.
    pub target: &'static str,
}

pub async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        version: crate::RELEASE_TAG,
        target: crate::TARGET_TRIPLE,
    })
}
