use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};
use tower_http::cors::CorsLayer;
use tower_http::trace::{DefaultMakeSpan, TraceLayer};
use tracing::Level;

use crate::state::AppState;

mod actions;
mod classes;
mod events;
mod health;
mod inventory;
mod objects;
mod settings;
mod state;

/// Build the axum router.
///
/// dobjd is API-only — the UI is served separately (Vite on `:1420` in dev,
/// Tauri's webview for the desktop app).
///
/// Note: axum routes literal paths (e.g. `/objects/dir`) before
/// parameterized ones (`/objects/{file_name}`), so the relative order
/// isn't load-bearing — but the literals are listed first for readability.
pub fn router(app_state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/events", get(events::stream))
        .route("/actions/runs/{run_id}/events", get(actions::run_events))
        .route("/inventory", get(inventory::load_inventory))
        .route("/state-root", get(state::get_state_root))
        .route("/objects/dir", get(objects::get_objects_dir))
        .route("/objects/import", post(objects::import_object))
        .route("/objects/{file_name}", get(objects::inspect_object))
        .route("/classes", get(classes::list_classes))
        .route("/classes/{name}", get(classes::inspect_class))
        .route(
            "/settings",
            get(settings::get_settings).put(settings::put_settings),
        )
        .route("/actions", get(actions::list_actions))
        .route("/actions/run", post(actions::run_action))
        // Raw `.pexe` bytes; raise the body cap from axum's 2 MiB default to
        // the pexe limit so larger plugins aren't rejected before the daemon
        // can validate them.
        .route(
            "/actions/install",
            post(actions::install_plugin)
                .layer(DefaultBodyLimit::max(driver::MAX_PEXE_BYTES as usize)),
        )
        .route("/actions/runs/{run_id}", get(actions::get_run))
        .route("/actions/{id}", get(actions::inspect_action))
        .route("/actions/{id}/feasibility", get(actions::check_feasibility))
        .with_state(app_state)
        .layer(CorsLayer::permissive())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_failure(()),
        )
}
