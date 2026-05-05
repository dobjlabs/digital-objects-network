use std::path::PathBuf;

use axum::{
    Router,
    routing::{get, post},
};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

mod actions;
mod events;
mod inventory;
mod objects;
mod settings;
mod state;

/// Build the axum router.
///
/// `static_dir`, when set, makes dobjd serve a static frontend bundle (the
/// React build under `app-gui/dist`) at every path the API doesn't claim.
/// Without it, dobjd is API-only and a separate dev server (Vite) hosts the
/// UI.
pub fn router(app_state: AppState, static_dir: Option<PathBuf>) -> Router {
    let api = Router::new()
        .route("/inventory", get(inventory::load_inventory))
        .route("/state-root", get(state::get_state_root))
        .route("/objects/dir", get(objects::get_objects_dir))
        .route("/objects/parse", post(objects::parse_object))
        .route(
            "/settings",
            get(settings::get_settings).put(settings::put_settings),
        )
        .route("/actions/run", post(actions::run_action))
        .route("/events", get(events::stream))
        .with_state(app_state);

    let api = match static_dir {
        Some(dir) => api.fallback_service(ServeDir::new(dir)),
        None => api,
    };

    api.layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}
