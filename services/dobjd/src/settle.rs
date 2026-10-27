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

/// Interval between settlement passes while submissions remain unresolved.
pub const SETTLE_INTERVAL: Duration = Duration::from_millis(1500);
pub const UNATTEMPTED_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const MAX_SUBMISSION_RETRIES: usize = 4;

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
        let mut outcome = match self.driver.settle() {
            Ok(outcome) => outcome,
            Err(err) => {
                tracing::warn!("settlement unavailable, continuing local cleanup: {err:#}");
                let records = self.driver.list_transactions()?;
                SettleOutcome {
                    open_records: records.iter().filter(|record| record.is_open()).count(),
                    unattempted_records: records
                        .iter()
                        .filter(|record| record.is_open() && !record.needs_membership())
                        .count(),
                    ..SettleOutcome::default()
                }
            }
        };
        let pruned = self.driver.prune_transactions(driver::now_ms())?;
        outcome.pruned_tx_finals = pruned.tx_finals;
        outcome.next_cleanup_at_ms = pruned.next_cleanup_at_ms;
        outcome.objects.extend(pruned.objects);
        outcome.removed_files.extend(pruned.removed_files);
        self.publish(&outcome);
        Ok(outcome)
    }

    fn start_retries(self: &Arc<Self>) -> Result<()> {
        let records = self.driver.list_transactions()?;
        let due = self
            .submissions
            .lock()
            .expect("submission retry lock poisoned")
            .due(&records, Instant::now());
        for tx_final in due {
            let settler = self.clone();
            tokio::spawn(async move {
                let driver = settler.driver.clone();
                let key = tx_final.clone();
                let submission = tokio::task::spawn_blocking(move || {
                    driver.submit(&payload::decode_hash_hex(&key)?)
                })
                .await;
                let result = submission
                    .map_err(anyhow::Error::from)
                    .and_then(|result| result);
                settler
                    .submissions
                    .lock()
                    .expect("submission retry lock poisoned")
                    .complete(&tx_final, Instant::now(), result.is_ok());
                match result {
                    Ok(_) => {
                        let publisher = settler.clone();
                        if let Err(err) = tokio::task::spawn_blocking(move || {
                            publisher.publish_submission(&tx_final)
                        })
                        .await
                        {
                            tracing::error!("retry publication panicked: {err}");
                        }
                    }
                    Err(err) => tracing::warn!(%tx_final, "submission retry failed: {err:#}"),
                }
                settler.wake();
            });
        }
        Ok(())
    }

    fn publish_submission(&self, tx_final: &str) {
        let _pass = self.pass_lock.lock().expect("pass lock poisoned");
        let record = match payload::decode_hash_hex(tx_final)
            .and_then(|key| self.driver.get_transaction(&key))
        {
            Ok(record) => record,
            Err(err) => {
                tracing::warn!(%tx_final, "could not read resubmitted transaction: {err:#}");
                return;
            }
        };
        let files = record
            .input_files
            .iter()
            .chain(&record.output_files)
            .cloned()
            .collect::<Vec<_>>();
        let objects = self
            .driver
            .read_available_objects(&files)
            .unwrap_or_else(|err| {
                tracing::warn!("could not read resubmitted objects: {err:#}");
                Vec::new()
            });
        self.publish(&SettleOutcome {
            records: vec![record],
            objects,
            ..SettleOutcome::default()
        });
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
        for tx_final in &outcome.pruned_tx_finals {
            self.events.publish(Event::TransactionPruned {
                tx_final: tx_final.clone(),
            });
        }
    }

    /// Settle while transactions remain open; otherwise wait for a wake signal.
    pub async fn run_loop(self: Arc<Self>) {
        loop {
            let settler = self.clone();
            let delay = match tokio::task::spawn_blocking(move || {
                let outcome = settler.pass();
                if let Err(err) = settler.start_retries() {
                    tracing::warn!("could not schedule submission retries: {err:#}");
                }
                outcome
            })
            .await
            {
                Ok(Ok(outcome)) => {
                    if !outcome.records.is_empty() {
                        tracing::info!(changed = outcome.records.len(), "settlement pass");
                    }
                    next_pass_delay(&outcome, driver::now_ms())
                }
                Ok(Err(err)) => {
                    tracing::warn!("settlement pass failed: {err:#}");
                    Some(SETTLE_INTERVAL)
                }
                Err(err) => {
                    tracing::error!("settlement pass panicked: {err}");
                    Some(SETTLE_INTERVAL)
                }
            };
            if let Some(delay) = delay {
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = self.wake.notified() => {}
                }
            } else {
                self.wake.notified().await;
            }
        }
    }
}

fn next_pass_delay(outcome: &SettleOutcome, current_time_ms: u64) -> Option<Duration> {
    let poll_delay = if outcome.open_records > outcome.unattempted_records {
        Some(SETTLE_INTERVAL)
    } else if outcome.open_records > 0 {
        Some(UNATTEMPTED_CHECK_INTERVAL)
    } else {
        None
    };
    let cleanup_delay = outcome
        .next_cleanup_at_ms
        .map(|deadline| Duration::from_millis(deadline.saturating_sub(current_time_ms).max(1)));
    poll_delay.into_iter().chain(cleanup_delay).min()
}

#[derive(Default)]
struct SubmissionRetries {
    attempts: HashMap<String, SubmissionRetry>,
}

struct SubmissionRetry {
    next_attempt: Instant,
    delay: Duration,
    in_flight: bool,
}

impl SubmissionRetries {
    fn due(&mut self, records: &[TransactionRecord], now: Instant) -> Vec<String> {
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
            .retain(|tx_final, retry| retry.in_flight || pending_ids.contains(tx_final.as_str()));
        let mut capacity = MAX_SUBMISSION_RETRIES.saturating_sub(
            self.attempts
                .values()
                .filter(|retry| retry.in_flight)
                .count(),
        );
        let mut due = Vec::new();
        for record in pending {
            let retry = self
                .attempts
                .entry(record.tx_final.clone())
                .or_insert(SubmissionRetry {
                    next_attempt: now + SETTLE_INTERVAL,
                    delay: SETTLE_INTERVAL,
                    in_flight: false,
                });
            if capacity == 0 || retry.in_flight || now < retry.next_attempt {
                continue;
            }
            retry.in_flight = true;
            capacity -= 1;
            due.push(record.tx_final.clone());
        }
        due
    }

    fn complete(&mut self, tx_final: &str, now: Instant, succeeded: bool) {
        if succeeded {
            self.attempts.remove(tx_final);
        } else if let Some(retry) = self.attempts.get_mut(tx_final) {
            retry.in_flight = false;
            retry.delay = (retry.delay * 2).min(Duration::from_secs(60));
            retry.next_attempt = now + retry.delay;
        }
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
    fn idle_cleanup_prunes_receipts_with_a_distinct_event() {
        use driver::{DriverDeps, DriverPaths, Hash};
        let home = tempfile::tempdir().unwrap();
        let paths = DriverPaths::from_dobj_root(home.path());
        let key = payload::encode_hash_hex(&Hash::default());
        let mut record = record(&key);
        record.state = TransactionState::Expired;
        let settled_at = driver::now_ms();
        record.settled_at_ms = Some(settled_at);
        let directory = paths.transactions_dir.join(&key);
        std::fs::create_dir_all(&directory).unwrap();
        let record_path = directory.join(driver::paths::RECORD_FILE);
        std::fs::write(&record_path, serde_json::to_vec(&record).unwrap()).unwrap();
        std::fs::write(directory.join(driver::paths::PAYLOAD_FILE), b"proof").unwrap();
        let driver =
            Arc::new(Driver::open(paths.clone(), DriverDeps::load(&paths).unwrap()).unwrap());
        let events = EventHub::new();
        let mut updates = events.subscribe();
        let settler = Settler::new(driver, events);
        let retained = settler.pass().unwrap();
        assert_eq!(retained.open_records, 0);
        assert_eq!(
            retained.next_cleanup_at_ms,
            Some(settled_at + driver::TRANSACTION_RETENTION_MS)
        );
        assert!(retained.pruned_tx_finals.is_empty());
        record.settled_at_ms = Some(1);
        std::fs::write(&record_path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert_eq!(settler.pass().unwrap().pruned_tx_finals, vec![key.clone()]);
        assert!(!directory.exists());
        assert!(
            matches!(updates.try_recv().unwrap().event, Event::TransactionPruned { tx_final } if tx_final == key)
        );
    }

    #[test]
    fn idle_settler_wakes_for_retention_deadline() {
        let mut outcome = SettleOutcome::default();
        assert_eq!(next_pass_delay(&outcome, 1000), None);
        outcome.next_cleanup_at_ms = Some(6000);
        assert_eq!(
            next_pass_delay(&outcome, 1000),
            Some(Duration::from_secs(5))
        );
        outcome.open_records = 1;
        assert_eq!(next_pass_delay(&outcome, 1000), Some(SETTLE_INTERVAL));
        outcome.open_records = 0;
        assert_eq!(
            next_pass_delay(&outcome, 7000),
            Some(Duration::from_millis(1))
        );
    }

    #[test]
    fn submission_errors_wake_the_settler() {
        use driver::{DriverDeps, DriverPaths, Hash};
        use futures_util::FutureExt;
        let home = tempfile::tempdir().unwrap();
        let paths = DriverPaths::from_dobj_root(home.path());
        let driver =
            Arc::new(Driver::open(paths.clone(), DriverDeps::load(&paths).unwrap()).unwrap());
        let events = EventHub::new();
        let services = crate::state::Services {
            settler: Arc::new(Settler::new(driver.clone(), events.clone())),
            driver,
            events,
            runs: crate::runs::RunRegistry::default(),
            proof_jobs: crate::proof_jobs::ProofJobRegistry::default(),
        };
        assert!(services.submit_transaction(&Hash::default()).is_err());
        assert!(services.settler.wake.notified().now_or_never().is_some());
    }

    #[test]
    fn staged_transactions_poll_slowly_until_submission_is_attempted() {
        let mut outcome = SettleOutcome {
            open_records: 2,
            unattempted_records: 2,
            ..Default::default()
        };
        assert_eq!(
            next_pass_delay(&outcome, 0),
            Some(UNATTEMPTED_CHECK_INTERVAL)
        );
        outcome.unattempted_records = 1;
        assert_eq!(next_pass_delay(&outcome, 0), Some(SETTLE_INTERVAL));
    }

    #[test]
    fn cleanup_deadline_precedes_the_next_staged_poll() {
        let outcome = SettleOutcome {
            open_records: 1,
            unattempted_records: 1,
            next_cleanup_at_ms: Some(1000),
            ..SettleOutcome::default()
        };
        assert_eq!(next_pass_delay(&outcome, 0), Some(Duration::from_secs(1)));
    }

    #[test]
    fn failed_submissions_back_off_until_accepted() {
        let record = record("attempted");
        let records = [record];
        let mut retries = SubmissionRetries::default();
        let start = Instant::now();
        assert!(retries.due(&records, start).is_empty());
        assert_eq!(
            retries.due(&records, start + SETTLE_INTERVAL),
            vec!["attempted"]
        );
        retries.complete("attempted", start + SETTLE_INTERVAL, false);
        assert!(
            retries
                .due(&records, start + SETTLE_INTERVAL * 2)
                .is_empty()
        );
        assert_eq!(
            retries.due(&records, start + SETTLE_INTERVAL * 3),
            vec!["attempted"]
        );
        retries.complete("attempted", start + SETTLE_INTERVAL * 3, true);
        assert!(retries.attempts.is_empty());
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
        assert!(retries.due(&records, start).is_empty());
        assert_eq!(
            retries.due(&records, start + SETTLE_INTERVAL),
            vec!["attempted"]
        );
    }

    #[test]
    fn expiry_cancels_a_pending_retry() {
        let mut record = record("attempted");
        let mut retries = SubmissionRetries::default();
        let start = Instant::now();
        retries.due(&[record.clone()], start);
        assert_eq!(retries.attempts.len(), 1);
        record.state = TransactionState::Expired;
        assert!(
            retries
                .due(&[record], start + Duration::from_secs(120))
                .is_empty()
        );
        assert!(retries.attempts.is_empty());
    }

    #[test]
    fn retry_workers_are_bounded_and_not_scheduled_twice() {
        let records = (0..MAX_SUBMISSION_RETRIES + 2)
            .map(|index| record(&index.to_string()))
            .collect::<Vec<_>>();
        let mut retries = SubmissionRetries::default();
        let start = Instant::now();
        assert!(retries.due(&records, start).is_empty());
        let due = retries.due(&records, start + SETTLE_INTERVAL);
        assert_eq!(due.len(), MAX_SUBMISSION_RETRIES);
        assert!(
            retries
                .due(&records, start + SETTLE_INTERVAL * 2)
                .is_empty()
        );
        retries.complete(&due[0], start + SETTLE_INTERVAL * 2, false);
        assert_eq!(
            retries.due(&records, start + SETTLE_INTERVAL * 2),
            vec![MAX_SUBMISSION_RETRIES.to_string()]
        );
    }

    #[tokio::test]
    async fn synchronizer_failure_still_prunes_and_preserves_staged_polling_cadence() {
        use axum::{Router, http::StatusCode, routing::get};
        use driver::{DriverDeps, DriverPaths};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/v1/state/head",
            get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        tokio::task::spawn_blocking(move || {
            let home = tempfile::tempdir().unwrap();
            let paths = DriverPaths::from_dobj_root(home.path());
            let mut staged = record("staged");
            staged.submit_attempted_at_ms = None;
            let expired_key = payload::encode_hash_hex(&driver::Hash::default());
            let mut expired = record(&expired_key);
            expired.state = TransactionState::Expired;
            expired.settled_at_ms = Some(1);
            for record in [staged, expired] {
                let directory = paths.transactions_dir.join(&record.tx_final);
                std::fs::create_dir_all(&directory).unwrap();
                std::fs::write(directory.join(driver::paths::RECORD_FILE), serde_json::to_vec(&record).unwrap()).unwrap();
                std::fs::write(directory.join(driver::paths::PAYLOAD_FILE), b"proof").unwrap();
            }
            let driver = Arc::new(Driver::open(paths.clone(), DriverDeps::load(&paths).unwrap()).unwrap());
            let mut settings = driver.load_settings().unwrap();
            settings.synchronizer_api_url = url;
            driver.save_settings(&settings).unwrap();
            let events = EventHub::new();
            let mut updates = events.subscribe();
            let settler = Settler::new(driver.clone(), events);
            let outcome = settler.pass().unwrap();
            assert_eq!(outcome.open_records, 1);
            assert_eq!(outcome.unattempted_records, 1);
            assert_eq!(next_pass_delay(&outcome, driver::now_ms()), Some(UNATTEMPTED_CHECK_INTERVAL));
            assert_eq!(outcome.pruned_tx_finals, vec![expired_key.clone()]);
            assert_eq!(driver.list_transactions().unwrap().len(), 1);
            assert!(matches!(updates.try_recv().unwrap().event, Event::TransactionPruned { tx_final } if tx_final == expired_key));
        }).await.unwrap();
        server.abort();
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
        let (entered, waiting) = std::sync::mpsc::channel();
        let release = Arc::new(Notify::new());
        let release_request = release.clone();
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
                    let entered = entered.clone();
                    let release_request = release_request.clone();
                    async move {
                        if requests.fetch_add(1, Ordering::SeqCst) == 0 {
                            return StatusCode::SERVICE_UNAVAILABLE.into_response();
                        }
                        entered.send(()).unwrap();
                        release_request.notified().await;
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
            let settler = Arc::new(Settler::new(driver.clone(), events));
            settler.pass().unwrap();
            settler.start_retries().unwrap();
            std::thread::sleep(SETTLE_INTERVAL);
            settler.start_retries().unwrap();
            waiting.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(settler.pass().unwrap().open_records, 1);
            assert!(settler.sync_objects(None).unwrap().is_empty());
            release.notify_one();
            let event = tokio::runtime::Handle::current().block_on(async {
                tokio::time::timeout(Duration::from_secs(5), updates.recv()).await.unwrap().unwrap()
            });
            assert!(matches!(event.event, Event::Transaction(record) if record.state == TransactionState::Submitted));
            assert_eq!(driver.get_transaction(&tx_final).unwrap().relayer_job_id.as_deref(), Some("recovered-job"));
            assert_eq!(submissions.load(Ordering::SeqCst), 2);
        }).await.unwrap();
        server.abort();
    }
}
