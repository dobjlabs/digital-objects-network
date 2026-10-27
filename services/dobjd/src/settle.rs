//! Serialized settlement passes for open transactions and object synchronization.
//! Publish all changes through the event hub.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use driver::{Driver, ObjectQuery, SettleOutcome};
use tokio::sync::Notify;
use wire_types::ObjectSummary;
use wire_types::transactions::{TransactionRecord, TransactionState};

use crate::events::{Event, EventHub};

/// Interval between settlement passes while transactions remain open.
pub const SETTLE_INTERVAL: Duration = Duration::from_millis(1500);

pub struct Settler {
    driver: Arc<Driver>,
    events: EventHub,
    /// Prevent overlapping background and client-requested settlement passes.
    pass_lock: Mutex<()>,
    submissions: Mutex<SubmissionRetries>,
    wake: Notify,
}

impl Settler {
    pub fn new(driver: Arc<Driver>, events: EventHub) -> Self {
        Self {
            driver,
            events,
            pass_lock: Mutex::new(()),
            submissions: Mutex::new(SubmissionRetries::default()),
            wake: Notify::new(),
        }
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }

    /// Reconcile open transactions and publish changes. Blocks on network requests.
    pub fn pass(&self) -> Result<SettleOutcome> {
        let _pass = self.pass_lock.lock().expect("pass lock poisoned");
        let mut outcome = self.driver.settle()?;
        let records = self.driver.list_transactions()?;
        let submitted = self
            .submissions
            .lock()
            .expect("submission retry lock poisoned")
            .retry(&records, Instant::now(), |record| {
                self.driver
                    .submit(&payload::decode_hash_hex(&record.tx_final)?)
            });
        for record in submitted {
            let files = record
                .input_files
                .iter()
                .chain(&record.output_files)
                .cloned()
                .collect::<Vec<_>>();
            match self.driver.read_available_objects(&files) {
                Ok(objects) => outcome.objects.extend(objects),
                Err(err) => tracing::warn!("could not read resubmitted objects: {err:#}"),
            }
            outcome.records.push(record);
        }
        self.publish(&outcome);
        Ok(outcome)
    }

    /// Synchronize objects, falling back to a local listing if settlement fails.
    /// Blocks on network requests.
    pub fn sync_objects(&self, query: Option<&ObjectQuery>) -> Result<Vec<ObjectSummary>> {
        let _pass = self.pass_lock.lock().expect("pass lock poisoned");
        match self.driver.sync_objects(query) {
            Ok(synced) => {
                self.publish(&synced.outcome);
                Ok(synced.objects)
            }
            Err(err) => {
                tracing::warn!("sync_objects failed, listing from disk: {err:#}");
                self.driver.list_objects(query)
            }
        }
    }

    fn publish(&self, outcome: &SettleOutcome) {
        for record in &outcome.records {
            self.events.publish(Event::Transaction(record.clone()));
        }
        for file_name in &outcome.removed_files {
            self.events.publish(Event::ObjectRemoved {
                file_name: file_name.clone(),
            });
        }
        for object in &outcome.objects {
            self.events.publish(Event::Object(object.clone()));
        }
    }

    /// Settle while transactions remain open; otherwise wait for a wake signal.
    pub async fn run_loop(self: Arc<Self>) {
        loop {
            let settler = self.clone();
            let open_records = match tokio::task::spawn_blocking(move || settler.pass()).await {
                Ok(Ok(outcome)) => {
                    if !outcome.records.is_empty() {
                        tracing::info!(changed = outcome.records.len(), "settlement pass");
                    }
                    outcome.open_records
                }
                Ok(Err(err)) => {
                    tracing::warn!("settlement pass failed: {err:#}");
                    1
                }
                Err(err) => {
                    tracing::error!("settlement pass panicked: {err}");
                    1
                }
            };
            if open_records == 0 {
                self.wake.notified().await;
                continue;
            }
            tokio::select! {
                _ = tokio::time::sleep(SETTLE_INTERVAL) => {}
                _ = self.wake.notified() => {}
            }
        }
    }
}

#[derive(Default)]
struct SubmissionRetries {
    attempts: HashMap<String, SubmissionRetry>,
}

struct SubmissionRetry {
    next_attempt: Instant,
    delay: Duration,
}

impl SubmissionRetries {
    fn retry(
        &mut self,
        records: &[TransactionRecord],
        now: Instant,
        mut submit: impl FnMut(&TransactionRecord) -> Result<TransactionRecord>,
    ) -> Vec<TransactionRecord> {
        let pending = records
            .iter()
            .filter(|record| {
                record.state == TransactionState::Recorded
                    && record.submit_attempted_at_ms.is_some()
            })
            .collect::<Vec<_>>();
        let pending_ids = pending
            .iter()
            .map(|record| record.tx_final.as_str())
            .collect::<HashSet<_>>();
        self.attempts
            .retain(|tx_final, _| pending_ids.contains(tx_final.as_str()));
        let mut submitted = Vec::new();
        for record in pending {
            let retry = self
                .attempts
                .entry(record.tx_final.clone())
                .or_insert(SubmissionRetry {
                    next_attempt: now + SETTLE_INTERVAL,
                    delay: SETTLE_INTERVAL,
                });
            if now < retry.next_attempt {
                continue;
            }
            match submit(record) {
                Ok(updated) => {
                    self.attempts.remove(&record.tx_final);
                    submitted.push(updated);
                }
                Err(err) => {
                    retry.delay = (retry.delay * 2).min(Duration::from_secs(60));
                    retry.next_attempt = now + retry.delay;
                    tracing::warn!(tx_final = %record.tx_final, "submission retry failed: {err:#}");
                }
            }
        }
        submitted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wire_types::QualifiedName;

    fn record(tx_final: &str) -> TransactionRecord {
        let mut record = TransactionRecord::recorded(
            tx_final.to_string(),
            QualifiedName::new("test", "Action"),
            "run".to_string(),
            vec![],
            vec![],
            "root".to_string(),
            1,
            vec![],
            vec![],
            0,
        );
        record.submit_attempted_at_ms = Some(1);
        record
    }

    #[test]
    fn failed_submissions_back_off_until_accepted() {
        let record = record("attempted");
        let records = [record];
        let mut retries = SubmissionRetries::default();
        let start = Instant::now();
        let mut calls = 0;
        let mut submit = |record: &TransactionRecord| {
            calls += 1;
            if calls == 1 {
                anyhow::bail!("relayer unavailable");
            }
            let mut updated = record.clone();
            updated.state = TransactionState::Submitted;
            updated.relayer_job_id = Some("existing-job".to_string());
            Ok(updated)
        };
        assert!(retries.retry(&records, start, &mut submit).is_empty());
        assert!(
            retries
                .retry(&records, start + SETTLE_INTERVAL, &mut submit)
                .is_empty()
        );
        assert!(
            retries
                .retry(&records, start + SETTLE_INTERVAL * 2, &mut submit)
                .is_empty()
        );
        let submitted = retries.retry(&records, start + SETTLE_INTERVAL * 3, &mut submit);
        assert_eq!(submitted.len(), 1);
        assert_eq!(submitted[0].relayer_job_id.as_deref(), Some("existing-job"));
        assert_eq!(calls, 2);
        assert!(retries.attempts.is_empty());
        assert!(
            retries
                .retry(&submitted, start + Duration::from_secs(120), |_| panic!(
                    "already submitted"
                ))
                .is_empty()
        );
    }

    #[test]
    fn restart_retries_attempted_records_but_never_unsubmitted_or_terminal_records() {
        let mut unsubmitted = record("unsubmitted");
        unsubmitted.submit_attempted_at_ms = None;
        let mut expired = record("expired");
        expired.state = TransactionState::Expired;
        let mut landed = record("landed");
        landed.state = TransactionState::Landed;
        let records = [record("attempted"), unsubmitted, expired, landed];
        let mut retries = SubmissionRetries::default();
        let start = Instant::now();
        assert!(
            retries
                .retry(&records, start, |_| panic!("initial backoff"))
                .is_empty()
        );
        let submitted = retries.retry(&records, start + SETTLE_INTERVAL, |record| {
            assert_eq!(record.tx_final, "attempted");
            let mut record = record.clone();
            record.state = TransactionState::Submitted;
            Ok(record)
        });
        assert_eq!(submitted.len(), 1);
    }

    #[test]
    fn expiry_cancels_a_pending_retry() {
        let mut record = record("attempted");
        let mut retries = SubmissionRetries::default();
        let start = Instant::now();
        retries.retry(&[record.clone()], start, |_| panic!("initial backoff"));
        assert_eq!(retries.attempts.len(), 1);
        record.state = TransactionState::Expired;
        retries.retry(&[record], start + Duration::from_secs(120), |_| {
            panic!("expired")
        });
        assert!(retries.attempts.is_empty());
    }

    #[tokio::test]
    async fn settlement_recovers_failed_submission_after_driver_restart() {
        use axum::{
            Json, Router,
            http::StatusCode,
            response::IntoResponse,
            routing::{get, post},
        };
        use driver::{DriverDeps, DriverPaths, Hash};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wire_types::{
            relayer::{JobStatus, SubmitProofResponse},
            synchronizer::{MembershipResponse, StateHeadResponse},
        };

        let submissions = Arc::new(AtomicUsize::new(0));
        let requests = submissions.clone();
        let app = Router::new()
            .route(
                "/v1/state/head",
                get(|| async {
                    Json(StateHeadResponse {
                        slot: 1,
                        state_root: Hash::default(),
                        block_number: 1,
                        block_hash: Default::default(),
                        created_count: 0,
                        nullifier_count: 0,
                        state_root_count: 1,
                    })
                }),
            )
            .route(
                "/v1/state/membership",
                post(|| async {
                    Json(MembershipResponse {
                        block_number: 1,
                        block_hash: Default::default(),
                        state_root: Hash::default(),
                        created_results: vec![],
                        nullifier_results: vec![],
                    })
                }),
            )
            .route(
                "/api/v1/proofs",
                post(move || {
                    let requests = requests.clone();
                    async move {
                        if requests.fetch_add(1, Ordering::SeqCst) == 0 {
                            return StatusCode::SERVICE_UNAVAILABLE.into_response();
                        }
                        Json(SubmitProofResponse {
                            job_id: "recovered-job".to_string(),
                            status: JobStatus::Queued,
                            tx_final: Hash::default(),
                            state_root: Hash::default(),
                            attempt_count: 0,
                            created_at: 0,
                        })
                        .into_response()
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        tokio::task::spawn_blocking(move || {
            let home = tempfile::tempdir().unwrap();
            let paths = DriverPaths::from_dobj_root(home.path());
            let tx_final = Hash::default();
            let key = payload::encode_hash_hex(&tx_final);
            let mut record = record(&key);
            record.submit_attempted_at_ms = None;
            record.created.push(key.clone());
            let directory = paths.transactions_dir.join(&key);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join(driver::paths::RECORD_FILE), serde_json::to_vec(&record).unwrap()).unwrap();
            std::fs::write(directory.join(driver::paths::PAYLOAD_FILE), b"proof").unwrap();
            let driver = Driver::open(paths.clone(), DriverDeps::load(&paths).unwrap()).unwrap();
            let mut settings = driver.load_settings().unwrap();
            settings.synchronizer_api_url = url.clone();
            settings.relayer_api_url = url;
            driver.save_settings(&settings).unwrap();
            assert!(driver.submit(&tx_final).is_err());
            assert!(driver.get_transaction(&tx_final).unwrap().submit_attempted_at_ms.is_some());
            assert!(driver.delete_transaction(&tx_final).is_err());
            drop(driver);

            let driver = Arc::new(Driver::open(paths.clone(), DriverDeps::load(&paths).unwrap()).unwrap());
            let events = EventHub::new();
            let mut updates = events.subscribe();
            let settler = Settler::new(driver.clone(), events);
            settler.pass().unwrap();
            std::thread::sleep(SETTLE_INTERVAL);
            let outcome = settler.pass().unwrap();
            assert_eq!(outcome.records.len(), 1);
            assert_eq!(driver.get_transaction(&tx_final).unwrap().relayer_job_id.as_deref(), Some("recovered-job"));
            assert!(matches!(updates.try_recv().unwrap().event, Event::Transaction(record) if record.state == TransactionState::Submitted));
            assert_eq!(submissions.load(Ordering::SeqCst), 2);
        }).await.unwrap();
        server.abort();
    }
}
