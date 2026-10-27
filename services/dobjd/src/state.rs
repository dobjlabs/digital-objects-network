//! Services shared by HTTP, MCP, and background workers.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use driver::{Driver, DriverError, Hash};
use wire_types::transactions::TransactionRecord;
use wire_types::{ObjectSummary, ProofJobState, WaitQuery};

use crate::error::ServiceError;
use crate::events::{Event, EventHub};
use crate::mcp::McpRuntime;
use crate::proof_jobs::ProofJobRegistry;
use crate::routes::wait::wait_until;
use crate::runs::RunRegistry;
use crate::settle::Settler;

#[derive(Clone)]
pub struct Services {
    pub driver: Arc<Driver>,
    pub events: EventHub,
    pub runs: RunRegistry,
    pub proof_jobs: ProofJobRegistry,
    pub settler: Arc<Settler>,
}

#[derive(Clone)]
pub struct AppState {
    pub services: Services,
    pub mcp: Arc<McpRuntime>,
}

impl AppState {
    pub fn new(services: Services, mcp: Arc<McpRuntime>) -> Self {
        Self { services, mcp }
    }
}

pub fn parse_tx_final(hex: &str) -> Result<Hash, DriverError> {
    payload::decode_hash_hex(hex)
        .map_err(|err| DriverError::InvalidInput(format!("txFinal {hex}: {err}")))
}

impl Services {
    pub fn object_summary(&self, file_name: &str) -> Result<ObjectSummary> {
        self.driver.read_object(Path::new(file_name))
    }

    pub fn publish_objects(&self, file_names: &[String]) {
        if file_names.is_empty() {
            return;
        }
        match self.driver.read_available_objects(file_names) {
            Ok(summaries) => {
                for summary in summaries {
                    self.events.publish(Event::Object(summary));
                }
            }
            Err(err) => tracing::debug!("could not read object summaries: {err:#}"),
        }
    }

    pub fn announce_transaction(&self, record: &TransactionRecord) {
        self.events.publish(Event::Transaction(record.clone()));
        let file_names = record
            .output_files
            .iter()
            .chain(&record.input_files)
            .cloned()
            .collect::<Vec<_>>();
        self.publish_objects(&file_names);
        self.settler.wake();
    }

    pub fn submit_transaction(&self, tx_final: &Hash) -> Result<TransactionRecord> {
        let record = self.driver.submit(tx_final)?;
        self.announce_transaction(&record);
        Ok(record)
    }

    pub fn list_proof_jobs(&self, active_only: bool) -> Vec<ProofJobState> {
        self.proof_jobs
            .entries(active_only)
            .iter()
            .map(|entry| entry.snapshot())
            .collect()
    }

    /// Submit transactions in order; stop at the first error without rolling
    /// back earlier submissions. Blocks on network requests.
    pub fn submit_transactions(&self, tx_finals: &[Hash]) -> Result<Vec<TransactionRecord>> {
        let mut records = Vec::with_capacity(tx_finals.len());
        for tx_final in tx_finals {
            records.push(self.submit_transaction(tx_final)?);
        }
        Ok(records)
    }

    pub fn delete_transaction(&self, tx_final: &Hash) -> Result<TransactionRecord> {
        let deleted = self.driver.delete_transaction(tx_final)?;
        self.events.publish(Event::TransactionDeleted {
            tx_final: deleted.record.tx_final.clone(),
        });
        for file_name in deleted.removed_files {
            self.events.publish(Event::ObjectRemoved { file_name });
        }
        self.publish_objects(&deleted.record.input_files);
        Ok(deleted.record)
    }

    /// Read a job's state, optionally waiting for completion or timeout.
    pub async fn wait_for_proof_job(
        &self,
        job_id: &str,
        query: WaitQuery,
    ) -> Result<ProofJobState> {
        let entry = self
            .proof_jobs
            .get(job_id)
            .ok_or_else(|| ServiceError::ProofJobNotFound(job_id.to_string()))?;
        wait_until(
            &self.events,
            query,
            || Ok(entry.snapshot()),
            |job| job.status.is_terminal(),
            |event| match event {
                Event::ProofJob(job) if job.job_id == job_id => Ok(Some(job.clone())),
                _ => Ok(None),
            },
        )
        .await
    }

    /// Read a transaction, optionally waiting for landing, expiry, or timeout.
    /// Fail if the transaction is deleted while waiting.
    pub async fn wait_for_transaction(
        &self,
        tx_final: &Hash,
        query: WaitQuery,
    ) -> Result<TransactionRecord> {
        let key = payload::encode_hash_hex(tx_final);
        wait_until(
            &self.events,
            query,
            || self.driver.get_transaction(tx_final),
            |record| record.state.is_terminal(),
            |event| transaction_event(&key, event),
        )
        .await
    }
}

/// Match transaction updates; fail when the awaited transaction is deleted.
pub fn transaction_event(tx_final: &str, event: &Event) -> Result<Option<TransactionRecord>> {
    match event {
        Event::Transaction(record) if record.tx_final == tx_final => Ok(Some(record.clone())),
        Event::TransactionDeleted { tx_final: deleted } if deleted == tx_final => {
            anyhow::bail!("transaction {tx_final} was deleted")
        }
        _ => Ok(None),
    }
}
