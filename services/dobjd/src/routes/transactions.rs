use anyhow::anyhow;
use axum::{
    Json,
    extract::{Path, Query, State},
};
use wire_types::transactions::TransactionRecord;
use wire_types::{SubmitTransactionsRequest, WaitQuery};

use crate::error::ApiResult;
use crate::state::{AppState, parse_tx_final};

/// `POST /transactions/submit`: submit records in order, stopping on the first error.
/// Return updated records when all submissions succeed.
pub async fn submit(
    State(state): State<AppState>,
    Json(request): Json<SubmitTransactionsRequest>,
) -> ApiResult<Json<Vec<TransactionRecord>>> {
    let tx_finals = request
        .tx_finals
        .iter()
        .map(|hex| parse_tx_final(hex))
        .collect::<Result<Vec<_>, _>>()?;
    let services = state.services.clone();
    let records = tokio::task::spawn_blocking(move || services.submit_transactions(&tx_finals))
        .await
        .map_err(|err| anyhow!("submit task panicked: {err}"))??;
    Ok(Json(records))
}

/// `GET /transactions`: every record on disk, oldest first.
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<TransactionRecord>>> {
    let driver = state.services.driver.clone();
    let records = tokio::task::spawn_blocking(move || driver.list_transactions())
        .await
        .map_err(|err| anyhow!("list_transactions task panicked: {err}"))??;
    Ok(Json(records))
}

/// `GET /transactions/{txFinal}`: return the record, optionally waiting for
/// landing, expiry, or timeout.
pub async fn get(
    State(state): State<AppState>,
    Path(tx_final): Path<String>,
    Query(query): Query<WaitQuery>,
) -> ApiResult<Json<TransactionRecord>> {
    let hash = parse_tx_final(&tx_final)?;
    Ok(Json(
        state.services.wait_for_transaction(&hash, query).await?,
    ))
}

/// `DELETE /transactions/{txFinal}`: remove the record and unknown outputs.
/// Return the removed record. Submission attempts prevent deletion (409).
pub async fn delete(
    State(state): State<AppState>,
    Path(tx_final): Path<String>,
) -> ApiResult<Json<TransactionRecord>> {
    let hash = parse_tx_final(&tx_final)?;
    let services = state.services.clone();
    let record = tokio::task::spawn_blocking(move || services.delete_transaction(&hash))
        .await
        .map_err(|err| anyhow!("delete task panicked: {err}"))??;
    Ok(Json(record))
}
