use std::convert::Infallible;
use std::time::Duration;

use anyhow::anyhow;
use axum::http::HeaderMap;
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event as SseEvent, KeepAlive, Sse},
    },
};
use tokio_stream::wrappers::ReceiverStream;
use wire_types::{
    ActionSummary, CheckActionReport, QualifiedName, RunAccepted, RunActionRequest, RunState,
};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// `POST /actions/run` — accept an action and return immediately with a run
/// handle. The proof + commit pipeline runs on a background worker; follow it
/// via `GET /actions/runs/{run_id}` (poll) or `/events` (SSE). Input/feasibility
/// errors are not reported here: they surface as the run's terminal `failed`
/// state, observable through either follow path.
pub async fn run_action(
    State(state): State<AppState>,
    Json(req): Json<RunActionRequest>,
) -> (StatusCode, Json<RunAccepted>) {
    let input = req.input;

    // Pass strings through verbatim — the driver extracts basenames via
    // `Path::file_name`, so an absolute path or a bare basename resolve to the
    // same managed file.
    let input_objects: Vec<String> = input
        .input_object_paths
        .iter()
        .map(|path| path.trim().to_string())
        .collect();

    let accepted = crate::runs::spawn_run(
        &state.runs,
        state.driver.clone(),
        state.events.clone(),
        input.action,
        input_objects,
    );
    (StatusCode::ACCEPTED, Json(accepted))
}

/// `GET /actions/runs/{run_id}` — current state of a run: status, the result
/// once it succeeds, an error if it fails, and the full ordered progress log.
/// This is the disconnect-recovery path; pollable until the run is reaped.
pub async fn get_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> ApiResult<Json<RunState>> {
    let entry = state
        .runs
        .get(&run_id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("unknown run: {run_id}")))?;
    Ok(Json(entry.snapshot()))
}

/// `GET /actions/runs/{run_id}/events` — per-run SSE stream. On connect it
/// replays the run's buffered progress (resuming after `Last-Event-ID` if
/// present), then tails live events until the run reaches a terminal state.
/// Each event's SSE `id` is its index in the run's progress log.
pub async fn run_events(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(entry) = state.runs.get(&run_id) else {
        return (StatusCode::NOT_FOUND, format!("unknown run: {run_id}")).into_response();
    };

    // Resume just past the last event the client acknowledged, so a reconnect
    // replays only what it missed rather than the whole log.
    let start = resume_start_index(&headers);

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<SseEvent, Infallible>>(64);
    tokio::spawn(async move {
        let mut next = start;
        // The progress log is in-memory; a short poll tails it without any
        // cross-thread wakeup machinery. Latency here is invisible next to
        // proof-generation timescales.
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let (events, terminal) = entry.events_from(next);
            for (index, progress) in events {
                let json = serde_json::to_string(&progress)
                    .expect("RunActionProgress must serialize to JSON");
                let event = SseEvent::default().id(index.to_string()).data(json);
                if tx.send(Ok(event)).await.is_err() {
                    return; // client disconnected
                }
                next = index + 1;
            }
            if terminal {
                return;
            }
            // Wait for the next poll tick, but stop immediately if the client
            // has disconnected — otherwise we'd keep polling for a dropped
            // receiver through a long quiet phase (e.g. proof generation).
            tokio::select! {
                _ = ticker.tick() => {}
                _ = tx.closed() => return,
            }
        }
    });

    Sse::new(ReceiverStream::new(rx))
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

/// `POST /actions/install` — install a `.pexe` (raw archive bytes in the
/// request body) into the actions dir and hot-reload the catalog, so the
/// plugin is usable without restarting dobjd. The catalog is validated before
/// the swap commits and a bad archive is removed, so a failed install leaves
/// the running catalog untouched. Returns the installed plugin name.
pub async fn install_plugin(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<String>> {
    let driver = state.driver.clone();
    let name = tokio::task::spawn_blocking(move || driver.install_plugin(&body))
        .await
        .map_err(|err| anyhow!("install_plugin task panicked: {err}"))??;
    Ok(Json(name))
}

fn resume_start_index(headers: &HeaderMap) -> usize {
    headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| raw.parse::<usize>().ok())
        .map(|last| last.saturating_add(1))
        .unwrap_or(0)
}

/// `GET /actions` — full catalog of every action the loaded plugins
/// declare. Pure local state; no synchronizer round-trip. Use this
/// instead of `/objects` when you only need the action list.
pub async fn list_actions(State(state): State<AppState>) -> ApiResult<Json<Vec<ActionSummary>>> {
    let driver = state.driver.clone();
    let actions = tokio::task::spawn_blocking(move || driver.list_actions(None))
        .await
        .map_err(|err| anyhow!("list_actions task panicked: {err}"))??;
    Ok(Json(actions))
}

/// `GET /actions/{id}` — one action detail with predicate source.
/// `id` is the canonical `plugin::name` form.
pub async fn inspect_action(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ActionSummary>> {
    let driver = state.driver.clone();
    let qname = QualifiedName::parse(&id).map_err(|err| anyhow!("{err}"))?;
    let action = tokio::task::spawn_blocking(move || driver.get_action(&qname))
        .await
        .map_err(|err| anyhow!("get_action task panicked: {err}"))??;
    Ok(Json(action))
}

/// `GET /actions/{id}/feasibility` — does the local objects have what
/// this action needs? Returns the report shape `Driver::check_action`
/// produces: `feasible` flag, the candidate objects we'd use, and any
/// missing input class names. `id` is the canonical `plugin::name` form.
pub async fn check_feasibility(
    State(state): State<AppState>,
    Path(action_id): Path<String>,
) -> ApiResult<Json<CheckActionReport>> {
    let driver = state.driver.clone();
    let qname = QualifiedName::parse(&action_id).map_err(|err| anyhow!("{err}"))?;
    let report = tokio::task::spawn_blocking(move || driver.check_action(&qname))
        .await
        .map_err(|err| anyhow!("check_feasibility task panicked: {err}"))??;
    Ok(Json(report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn resume_start_index_defaults_to_zero() {
        assert_eq!(resume_start_index(&HeaderMap::new()), 0);
    }

    #[test]
    fn resume_start_index_starts_after_last_seen_event() {
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", HeaderValue::from_static("41"));
        assert_eq!(resume_start_index(&headers), 42);
    }

    #[test]
    fn resume_start_index_does_not_wrap_on_max_usize() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "last-event-id",
            HeaderValue::from_str(&usize::MAX.to_string()).unwrap(),
        );
        assert_eq!(resume_start_index(&headers), usize::MAX);
    }
}
