//! Serialized settlement passes for open transactions and object synchronization.
//! Publish all changes through the event hub.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use driver::{Driver, ObjectQuery, SettleOutcome};
use tokio::sync::Notify;
use wire_types::ObjectSummary;

use crate::events::{Event, EventHub};

/// Interval between settlement passes while transactions remain open.
pub const SETTLE_INTERVAL: Duration = Duration::from_millis(1500);

pub struct Settler {
    driver: Arc<Driver>,
    events: EventHub,
    /// Prevent overlapping background and client-requested settlement passes.
    pass_lock: Mutex<()>,
    wake: Notify,
}

impl Settler {
    pub fn new(driver: Arc<Driver>, events: EventHub) -> Self {
        Self {
            driver,
            events,
            pass_lock: Mutex::new(()),
            wake: Notify::new(),
        }
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }

    /// Reconcile open transactions and publish changes. Blocks on network requests.
    pub fn pass(&self) -> Result<SettleOutcome> {
        let _pass = self.pass_lock.lock().expect("pass lock poisoned");
        let outcome = self.driver.settle()?;
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
