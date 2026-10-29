use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post},
};
use tower_http::trace::{DefaultMakeSpan, TraceLayer};
use tracing::Level;

use crate::state::AppState;
use crate::web_access::WebAccess;

mod actions;
mod classes;
mod events;
mod health;
mod objects;
mod proof_jobs;
mod settings;
mod state;
mod transactions;
pub mod wait;

/// Build the axum router.
///
/// The default UI is mounted at /ui/.
///
/// Note: axum routes literal paths (e.g. `/objects/dir`) before
/// parameterized ones (`/objects/{file_name}`), so the relative order
/// isn't load-bearing -- but the literals are listed first for readability.
pub fn router(app_state: AppState, access: WebAccess) -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/events", get(events::stream))
        .route("/objects", get(objects::load_objects))
        .route("/state-root", get(state::get_state_root))
        .route("/objects/dir", get(objects::get_objects_dir))
        .route("/objects/unspent", get(objects::load_unspent_objects))
        .route("/objects/import", post(objects::import_object))
        .route("/objects/{file_name}", get(objects::inspect_object))
        .route("/objects/{file_name}/raw", get(objects::read_object_raw))
        .route(
            "/objects/{file_name}/discard_unsafe",
            post(objects::discard_object_unsafe),
        )
        .route("/objects/{file_name}/rename", post(objects::rename_object))
        .route("/classes", get(classes::list_classes))
        .route("/classes/{name}", get(classes::inspect_class))
        .route(
            "/settings",
            get(settings::get_settings).put(settings::put_settings),
        )
        .route("/actions", get(actions::list_actions))
        .route("/runs", post(actions::start_run).get(actions::list_runs))
        .route("/runs/{run_id}", get(actions::get_run))
        .route("/runs/{run_id}/events", get(actions::run_events))
        // Raw `.pexe` bytes; raise the body cap from axum's 2 MiB default to
        // the pexe limit so larger plugins aren't rejected before the daemon
        // can validate them.
        .route(
            "/actions/install",
            post(actions::install_plugin)
                .layer(DefaultBodyLimit::max(driver::MAX_PEXE_BYTES as usize)),
        )
        .route("/actions/{id}", get(actions::inspect_action))
        .route("/actions/{id}/feasibility", get(actions::check_feasibility))
        .route("/proof-jobs", post(proof_jobs::start).get(proof_jobs::list))
        .route("/proof-jobs/{job_id}", get(proof_jobs::get))
        .route("/transactions/submit", post(transactions::submit))
        .route("/transactions", get(transactions::list))
        .route(
            "/transactions/{tx_final}",
            get(transactions::get).delete(transactions::delete),
        )
        .with_state(app_state)
        .merge(crate::ui::router())
        .layer(access.cors())
        .layer(middleware::from_fn_with_state(
            access,
            crate::web_access::guard,
        ))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_failure(()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use driver::{Driver, DriverDeps, DriverPaths};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::{
        events::EventHub, mcp::McpRuntime, proof_jobs::ProofJobRegistry, runs::RunRegistry,
        settle::Settler, state::Services,
    };

    async fn request(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("host", "127.0.0.1:7717")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    fn test_app(home: &std::path::Path) -> Router {
        let paths = DriverPaths::from_dobj_root(home);
        let driver =
            Arc::new(Driver::open(paths.clone(), DriverDeps::load(&paths).unwrap()).unwrap());
        let events = EventHub::new();
        let services = Services {
            settler: Arc::new(Settler::new(driver.clone(), events.clone())),
            driver,
            events,
            runs: RunRegistry::default(),
            proof_jobs: ProofJobRegistry::default(),
        };
        let mcp = Arc::new(McpRuntime::new(
            services.clone(),
            "127.0.0.1:7718".to_string(),
            7717,
        ));
        router(AppState::new(services, mcp), WebAccess::new(7717))
    }

    #[tokio::test]
    async fn renaming_a_missing_object_is_not_found() {
        let home = tempfile::tempdir().unwrap();
        let app = test_app(home.path());
        let (status, body) = request(
            &app,
            "POST",
            "/objects/no%20such.dobj/rename",
            json!({"fileName": "new name"}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body["error"].as_str().unwrap().contains("no such.dobj"));
    }

    #[tokio::test]
    async fn resource_routes_accept_work_and_expose_its_lifecycle() {
        let home = tempfile::tempdir().unwrap();
        let app = test_app(home.path());
        for path in ["/runs", "/proof-jobs", "/transactions"] {
            assert_eq!(
                request(&app, "GET", path, Value::Null).await,
                (StatusCode::OK, json!([]))
            );
        }
        assert_eq!(
            request(
                &app,
                "POST",
                "/transactions/submit",
                json!({"txFinals": []})
            )
            .await,
            (StatusCode::OK, json!([]))
        );

        let input =
            json!({"action": {"pluginName": "test", "name": "Missing"}, "inputObjectPaths": []});
        let (status, job) = request(&app, "POST", "/proof-jobs", input.clone()).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let path = format!(
            "/proof-jobs/{}?wait=true&timeoutMs=5000",
            job["jobId"].as_str().unwrap()
        );
        let (status, job) = request(&app, "GET", &path, Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(job["status"], "failed");
        assert!(job["error"].as_str().unwrap().contains("unknown action"));

        let (status, run) = request(&app, "POST", "/runs", json!({"input": input})).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let path = format!("/runs/{}", run["runId"].as_str().unwrap());
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let (status, run) = request(&app, "GET", &path, Value::Null).await;
                assert_eq!(status, StatusCode::OK);
                if run["status"] == "failed" {
                    assert!(run["error"].as_str().unwrap().contains("unknown action"));
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
