//! Liveness probe for `dobj start` / `dobj status`.
//!
//! Returning 200 from this endpoint is itself the signal: dobjd's HTTP
//! listener only binds *after* `Driver::open_default()` succeeds in main —
//! plugin catalog loaded, RocksDB opened, paths resolved. So a successful
//! response here means the daemon initialized cleanly. No further work is
//! needed in the handler.
//!
//! Matches the wire shape of the synchronizer/relayer `/healthz` so any
//! tooling that probes all three uses one parser.

use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct HealthResponse {
    pub ok: bool,
}

pub async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}
