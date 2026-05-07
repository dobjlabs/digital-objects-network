use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

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
/// Note: axum routes literal paths (e.g. `/objects/dir`, `/objects/parse`)
/// before parameterized ones (`/objects/{id}`), so the relative order isn't
/// load-bearing — but the literals are listed first for readability.
pub fn router(app_state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/inventory", get(inventory::load_inventory))
        .route("/state-root", get(state::get_state_root))
        .route("/objects/dir", get(objects::get_objects_dir))
        // `/objects/parse` accepts a `.dobj` upload — cap at the route
        // level so a hostile / buggy client can't OOM the daemon by
        // streaming an unbounded multipart body. axum's default limit is
        // ~2 MiB but doesn't cover Multipart unless we attach this layer.
        .route(
            "/objects/parse",
            post(objects::parse_object)
                .layer(DefaultBodyLimit::max(objects::MAX_DOBJ_UPLOAD_BYTES)),
        )
        .route("/objects/{id}", get(objects::inspect_object))
        .route("/classes", get(classes::list_classes))
        .route("/classes/{name}", get(classes::inspect_class))
        .route(
            "/settings",
            get(settings::get_settings).put(settings::put_settings),
        )
        .route("/actions", get(actions::list_actions))
        .route("/actions/run", post(actions::run_action))
        .route("/actions/{id}/feasibility", get(actions::check_feasibility))
        .route("/events", get(events::stream))
        .with_state(app_state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}
