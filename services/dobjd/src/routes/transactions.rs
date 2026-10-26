use anyhow::anyhow;
use axum::{
    Json,
    extract::{Path, Query, State},
};
use driver::{DriverError, Hash};
use wire_types::transactions::TransactionRecord;
use wire_types::{SubmitRequest, WaitQuery};

use crate::error::ApiResult;
use crate::events::Event;
use crate::routes::wait::wait_until;
use crate::state::{AppState, Services};

fn parse_tx_final(hex: &str) -> Result<Hash, DriverError> {
    payload::decode_hash_hex(hex)
        .map_err(|err| DriverError::InvalidInput(format!("txFinal {hex}: {err}")))
}

/// `POST /submissions`: submit records in order, stopping on the first error.
/// Return updated records when all submissions succeed.
pub async fn submit(
    State(state): State<AppState>,
    Json(request): Json<SubmitRequest>,
) -> ApiResult<Json<Vec<TransactionRecord>>> {
    let tx_finals = request
        .tx_finals
        .iter()
        .map(|hex| parse_tx_final(hex))
        .collect::<Result<Vec<_>, _>>()?;
    let services = state.services.clone();
    let records = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<TransactionRecord>> {
        let mut records = Vec::with_capacity(tx_finals.len());
        for tx_final in &tx_finals {
            let record = services.driver.submit(tx_final)?;
            services.announce_transaction(&record);
            records.push(record);
        }
        Ok(records)
    })
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
    let driver = state.services.driver.clone();
    let key = payload::encode_hash_hex(&hash);
    let record = wait_until(
        &state.services.events,
        query,
        move || driver.get_transaction(&hash),
        |record| record.state.is_terminal(),
        |event| match event {
            Event::Transaction(record) if record.tx_final == key => Some(record.clone()),
            _ => None,
        },
    )
    .await?;
    Ok(Json(record))
}

/// `DELETE /transactions/{txFinal}`: remove the record and unknown outputs.
/// Return the removed record. Submission attempts prevent deletion (409).
pub async fn delete(
    State(state): State<AppState>,
    Path(tx_final): Path<String>,
) -> ApiResult<Json<TransactionRecord>> {
    let hash = parse_tx_final(&tx_final)?;
    let services = state.services.clone();
    let record = tokio::task::spawn_blocking(move || delete_blocking(&services, &hash))
        .await
        .map_err(|err| anyhow!("delete task panicked: {err}"))??;
    Ok(Json(record))
}

fn delete_blocking(services: &Services, tx_final: &Hash) -> anyhow::Result<TransactionRecord> {
    let record = services.driver.delete_transaction(tx_final)?;
    services.events.publish(Event::TransactionDeleted {
        tx_final: record.tx_final.clone(),
    });
    for file_name in &record.output_files {
        if !services.driver.paths().objects_dir.join(file_name).exists() {
            services.events.publish(Event::ObjectRemoved {
                file_name: file_name.clone(),
            });
        }
    }
    services.publish_objects(&record.input_files);
    Ok(record)
}
