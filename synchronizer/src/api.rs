use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    http::{header::CONTENT_TYPE, HeaderValue, Method, StatusCode},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use hex::FromHex;
use pod2::middleware::Hash;
use synchronizer::api_types::{
    DashboardRecentSlotsResponse, DashboardSlotStatus, DashboardStatus, DashboardSummaryResponse,
    HealthResponse, StateFullResponse, StateHeadResponse, SyncProgressResponse, TxContainsEntry,
    TxContainsRequest, TxContainsResponse, TxStatusResponse,
};
use tokio::sync::watch;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::{info, warn};

use crate::clients::beacon::types::BlockId;
use crate::node::Node;
use common::encode_hash_hex;

#[derive(Clone)]
struct AppState {
    node: Arc<Node>,
}

struct HeadSnapshot {
    last_processed_slot: Option<u32>,
    last_processed_block_number: Option<u32>,
    current_gsr: Option<String>,
    current_block_number: Option<i64>,
    tx_count: usize,
    nullifier_count: usize,
    gsr_count: usize,
}

#[derive(serde::Deserialize)]
struct RecentSlotsQuery {
    limit: Option<usize>,
}

pub async fn run_api_server(
    node: Arc<Node>,
    bind_addr: SocketAddr,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let mut app = Router::new()
        .route("/healthz", get(healthz))
        .route("/sync-progress", get(get_sync_progress))
        .route("/v1/state/head", get(get_state_head))
        .route("/v1/state/full", get(get_state_full))
        .route("/v1/state/tx/contains", post(post_state_tx_contains))
        .route("/v1/state/tx/{tx_hash}", get(get_state_tx))
        .route("/v1/dashboard/summary", get(get_dashboard_summary))
        .route(
            "/v1/dashboard/recent-slots",
            get(get_dashboard_recent_slots),
        )
        .with_state(AppState {
            node: Arc::clone(&node),
        });

    if !node.config.cors_allowed_origins.is_empty() {
        app = app.layer(build_cors_layer(&node.config.cors_allowed_origins)?);
    }

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    info!(%bind_addr, "API server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            wait_for_shutdown(&mut shutdown_rx).await;
        })
        .await?;
    Ok(())
}

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

async fn get_sync_progress(
    State(app_state): State<AppState>,
) -> Result<Json<SyncProgressResponse>, (StatusCode, String)> {
    let progress = app_state
        .node
        .sync_db
        .last_progress()
        .await
        .map_err(internal_error)?;
    let (last_processed_slot, last_processed_block_number) = match progress {
        Some(progress) => (
            Some(progress.last_processed_slot),
            progress.last_processed_block_number,
        ),
        None => (None, None),
    };

    Ok(Json(SyncProgressResponse {
        last_processed_slot,
        last_processed_block_number,
    }))
}

async fn get_state_head(
    State(app_state): State<AppState>,
) -> Result<Json<StateHeadResponse>, (StatusCode, String)> {
    let head = build_head_snapshot(&app_state).await?;
    Ok(Json(StateHeadResponse {
        last_processed_slot: head.last_processed_slot,
        last_processed_block_number: head.last_processed_block_number,
        current_gsr: head.current_gsr,
        current_block_number: head.current_block_number,
        tx_count: head.tx_count,
        nullifier_count: head.nullifier_count,
        gsr_count: head.gsr_count,
    }))
}

async fn get_state_full(
    State(app_state): State<AppState>,
) -> Result<Json<StateFullResponse>, (StatusCode, String)> {
    let snapshot = app_state
        .node
        .state_machine
        .api_state_snapshot()
        .map_err(internal_error)?;

    let mut transactions = snapshot
        .transactions
        .iter()
        .map(encode_hash_hex)
        .collect::<Vec<_>>();
    transactions.sort();

    let mut nullifiers = snapshot
        .nullifiers
        .iter()
        .map(encode_hash_hex)
        .collect::<Vec<_>>();
    nullifiers.sort();

    let prior_gsrs = if snapshot.global_state_roots.is_empty() {
        Vec::new()
    } else {
        snapshot.global_state_roots[..snapshot.global_state_roots.len() - 1].to_vec()
    };
    let gsrs = prior_gsrs.iter().map(encode_hash_hex).collect::<Vec<_>>();

    Ok(Json(StateFullResponse {
        block_number: snapshot.current_block_number.unwrap_or(0),
        current_gsr: snapshot.current_gsr.as_ref().map(encode_hash_hex),
        transactions,
        nullifiers,
        gsrs,
    }))
}

async fn post_state_tx_contains(
    State(app_state): State<AppState>,
    Json(body): Json<TxContainsRequest>,
) -> Result<Json<TxContainsResponse>, (StatusCode, String)> {
    let hashes = body
        .tx_hashes
        .iter()
        .map(|raw| parse_hash_hex(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let present = app_state
        .node
        .state_machine
        .tx_exists_batch(&hashes)
        .map_err(internal_error)?;
    let head = build_head_snapshot(&app_state).await?;

    let results = hashes
        .iter()
        .zip(present.into_iter())
        .map(|(hash, present)| TxContainsEntry {
            tx_hash: encode_hash_hex(hash),
            present,
        })
        .collect();

    Ok(Json(TxContainsResponse {
        last_processed_slot: head.last_processed_slot,
        current_gsr: head.current_gsr,
        results,
    }))
}

async fn get_state_tx(
    State(app_state): State<AppState>,
    Path(tx_hash): Path<String>,
) -> Result<Json<TxStatusResponse>, (StatusCode, String)> {
    let hash = parse_hash_hex(&tx_hash)?;
    let present = app_state
        .node
        .state_machine
        .tx_exists(&hash)
        .map_err(internal_error)?;
    let head = build_head_snapshot(&app_state).await?;
    Ok(Json(TxStatusResponse {
        tx_hash: encode_hash_hex(&hash),
        present,
        last_processed_slot: head.last_processed_slot,
        current_gsr: head.current_gsr,
    }))
}

async fn build_head_snapshot(app_state: &AppState) -> Result<HeadSnapshot, (StatusCode, String)> {
    let progress = app_state
        .node
        .sync_db
        .last_progress()
        .await
        .map_err(internal_error)?;
    let (last_processed_slot, last_processed_block_number) = match progress {
        Some(progress) => (
            Some(progress.last_processed_slot),
            progress.last_processed_block_number,
        ),
        None => (None, None),
    };

    let snapshot = app_state
        .node
        .state_machine
        .api_state_snapshot()
        .map_err(internal_error)?;

    Ok(HeadSnapshot {
        last_processed_slot,
        last_processed_block_number,
        current_gsr: snapshot.current_gsr.as_ref().map(encode_hash_hex),
        current_block_number: snapshot.current_block_number,
        tx_count: snapshot.transactions.len(),
        nullifier_count: snapshot.nullifiers.len(),
        gsr_count: snapshot.global_state_roots.len(),
    })
}

async fn get_dashboard_summary(
    State(app_state): State<AppState>,
) -> Result<Json<DashboardSummaryResponse>, (StatusCode, String)> {
    let snapshot = app_state
        .node
        .state_machine
        .api_state_snapshot()
        .map_err(internal_error)?;
    let cursor = app_state
        .node
        .sync_db
        .last_cursor_info()
        .await
        .map_err(internal_error)?;
    let recent_slots = app_state
        .node
        .sync_db
        .recent_canonical_slots(1)
        .await
        .map_err(internal_error)?;
    let pending_recovery_count = app_state
        .node
        .sync_db
        .pending_recovery_count()
        .await
        .map_err(internal_error)?;

    let (beacon_head_slot, beacon_head_block_number) =
        match app_state.node.beacon_cli.get_block(BlockId::Head).await {
            Ok(Some(block)) => (
                Some(block.slot),
                block
                    .execution_payload
                    .as_ref()
                    .map(|payload| payload.block_number),
            ),
            Ok(None) => (None, None),
            Err(err) => {
                warn!(?err, "Failed to fetch beacon head for dashboard summary");
                (None, None)
            }
        };

    let last_processed_slot = cursor.as_ref().and_then(|value| value.last_processed_slot);
    let last_processed_block_number = cursor
        .as_ref()
        .and_then(|value| value.last_processed_block_number);
    let cursor_updated_at = cursor
        .as_ref()
        .map(|value| value.updated_at.clone())
        .unwrap_or_default();
    let cursor_freshness_secs = cursor
        .as_ref()
        .map(|value| cursor_freshness_secs(value.updated_at_unix));
    let slot_lag = last_processed_slot
        .zip(beacon_head_slot)
        .map(|(last_slot, head_slot)| head_slot.saturating_sub(last_slot));
    let block_lag = last_processed_block_number
        .zip(beacon_head_block_number)
        .map(|(last_block, head_block)| head_block.saturating_sub(last_block));
    let latest_slot_pending = recent_slots
        .first()
        .is_some_and(|slot| slot.status == DashboardSlotStatus::Pending);
    let status = classify_dashboard_status(
        slot_lag,
        cursor_freshness_secs,
        pending_recovery_count,
        latest_slot_pending,
    );
    let status_reason = dashboard_status_reason(
        status,
        slot_lag,
        block_lag,
        cursor_freshness_secs,
        pending_recovery_count,
        latest_slot_pending,
        last_processed_slot,
    );

    Ok(Json(DashboardSummaryResponse {
        status,
        status_reason,
        last_processed_slot,
        beacon_head_slot,
        slot_lag,
        beacon_head_block_number,
        block_lag,
        last_processed_block_number,
        current_block_number: snapshot.current_block_number,
        current_gsr: snapshot.current_gsr.as_ref().map(encode_hash_hex),
        tx_count: snapshot.transactions.len(),
        nullifier_count: snapshot.nullifiers.len(),
        gsr_count: snapshot.global_state_roots.len(),
        pending_recovery_count,
        cursor_updated_at,
    }))
}

async fn get_dashboard_recent_slots(
    State(app_state): State<AppState>,
    Query(query): Query<RecentSlotsQuery>,
) -> Result<Json<DashboardRecentSlotsResponse>, (StatusCode, String)> {
    let limit = query.limit.unwrap_or(25).clamp(1, 100);
    let slots = app_state
        .node
        .sync_db
        .recent_canonical_slots(limit)
        .await
        .map_err(internal_error)?;
    Ok(Json(DashboardRecentSlotsResponse { slots }))
}

fn build_cors_layer(origins: &[String]) -> Result<CorsLayer> {
    let parsed = origins
        .iter()
        .map(|value| {
            HeaderValue::from_str(value)
                .map_err(|err| anyhow::anyhow!("invalid CORS origin `{value}`: {err}"))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([CONTENT_TYPE])
        .allow_origin(AllowOrigin::list(parsed)))
}

fn cursor_freshness_secs(updated_at_unix: i64) -> u64 {
    let now = Utc::now().timestamp();
    now.saturating_sub(updated_at_unix) as u64
}

fn classify_dashboard_status(
    slot_lag: Option<u32>,
    cursor_freshness_secs: Option<u64>,
    pending_recovery_count: usize,
    latest_slot_pending: bool,
) -> DashboardStatus {
    if pending_recovery_count > 0 || latest_slot_pending {
        return DashboardStatus::Recovering;
    }

    let Some(cursor_freshness_secs) = cursor_freshness_secs else {
        return DashboardStatus::Stalled;
    };

    match slot_lag {
        Some(lag) if lag <= 2 && cursor_freshness_secs <= 24 => DashboardStatus::Healthy,
        Some(lag) if lag <= 32 && cursor_freshness_secs <= 180 => DashboardStatus::Lagging,
        Some(_) => DashboardStatus::Stalled,
        None if cursor_freshness_secs <= 24 => DashboardStatus::Healthy,
        None if cursor_freshness_secs <= 180 => DashboardStatus::Lagging,
        None => DashboardStatus::Stalled,
    }
}

fn dashboard_status_reason(
    status: DashboardStatus,
    slot_lag: Option<u32>,
    block_lag: Option<u32>,
    cursor_freshness_secs: Option<u64>,
    pending_recovery_count: usize,
    latest_slot_pending: bool,
    last_processed_slot: Option<u32>,
) -> String {
    if pending_recovery_count > 0 {
        return format!("{pending_recovery_count} recovery item(s) pending");
    }

    if latest_slot_pending {
        return "latest canonical slot is still pending apply".to_string();
    }

    if last_processed_slot.is_none() {
        return "waiting for the first processed slot".to_string();
    }

    let lag_descriptor = describe_lag(block_lag, slot_lag);

    match (status, slot_lag, cursor_freshness_secs) {
        (DashboardStatus::Healthy, Some(0), Some(freshness)) => {
            format!("synchronizer is caught up and cursor updated {freshness}s ago")
        }
        (DashboardStatus::Healthy, Some(_), Some(freshness)) => {
            format!("synchronizer is {lag_descriptor} and cursor updated {freshness}s ago")
        }
        (DashboardStatus::Healthy, None, Some(freshness)) => {
            format!("beacon head unavailable; cursor updated {freshness}s ago")
        }
        (DashboardStatus::Lagging, Some(_), Some(freshness)) => {
            format!("synchronizer is {lag_descriptor}; cursor updated {freshness}s ago")
        }
        (DashboardStatus::Lagging, None, Some(freshness)) => {
            format!("beacon head unavailable; cursor updated {freshness}s ago")
        }
        (DashboardStatus::Stalled, Some(_), Some(freshness)) => format!(
            "synchronizer is not advancing normally: {lag_descriptor}; cursor last updated {freshness}s ago"
        ),
        (DashboardStatus::Stalled, None, Some(freshness)) => {
            format!(
                "synchronizer is not advancing normally: cursor last updated {freshness}s ago"
            )
        }
        (DashboardStatus::Stalled, _, None) => "sync cursor is not initialized".to_string(),
        (DashboardStatus::Recovering, _, _) => "recovery is in progress".to_string(),
        (DashboardStatus::Healthy, _, None) | (DashboardStatus::Lagging, _, None) => {
            "sync cursor freshness is unavailable".to_string()
        }
    }
}

fn describe_lag(block_lag: Option<u32>, slot_lag: Option<u32>) -> String {
    match (block_lag, slot_lag) {
        (Some(0), _) => "caught up to beacon head".to_string(),
        (None, Some(0)) => "caught up to beacon head".to_string(),
        (Some(blocks), Some(slots)) if blocks == slots => {
            format!("{blocks} block(s) behind beacon head")
        }
        (Some(blocks), Some(slots)) => {
            format!("{blocks} block(s) behind beacon head ({slots} slot(s))")
        }
        (Some(blocks), None) => format!("{blocks} block(s) behind beacon head"),
        (None, Some(slots)) => format!("{slots} slot(s) behind beacon head"),
        (None, None) => "behind the latest known head".to_string(),
    }
}

fn parse_hash_hex(value: &str) -> Result<Hash, (StatusCode, String)> {
    let trimmed = value.trim().strip_prefix("0x").unwrap_or(value.trim());
    Hash::from_hex(trimmed).map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid hash `{value}`: {err}"),
        )
    })
}

fn internal_error(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

async fn wait_for_shutdown(shutdown_rx: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown_rx.borrow() {
            break;
        }
        if shutdown_rx.changed().await.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_status_is_recovering_when_pending_work_exists() {
        let status = classify_dashboard_status(Some(0), Some(2), 1, false);
        assert_eq!(status, DashboardStatus::Recovering);
    }

    #[test]
    fn dashboard_status_is_healthy_with_small_lag_and_fresh_cursor() {
        let status = classify_dashboard_status(Some(2), Some(24), 0, false);
        assert_eq!(status, DashboardStatus::Healthy);
    }

    #[test]
    fn dashboard_status_is_lagging_when_cursor_is_fresh_but_more_behind() {
        let status = classify_dashboard_status(Some(12), Some(80), 0, false);
        assert_eq!(status, DashboardStatus::Lagging);
    }

    #[test]
    fn dashboard_status_is_stalled_when_cursor_is_old() {
        let status = classify_dashboard_status(Some(1), Some(181), 0, false);
        assert_eq!(status, DashboardStatus::Stalled);
    }

    #[test]
    fn dashboard_status_falls_back_to_cursor_freshness_without_beacon_head() {
        let status = classify_dashboard_status(None, Some(10), 0, false);
        assert_eq!(status, DashboardStatus::Healthy);

        let lagging = classify_dashboard_status(None, Some(120), 0, false);
        assert_eq!(lagging, DashboardStatus::Lagging);

        let stalled = classify_dashboard_status(None, Some(300), 0, false);
        assert_eq!(stalled, DashboardStatus::Stalled);
    }
}
