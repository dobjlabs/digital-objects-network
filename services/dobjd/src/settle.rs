//! Serialized settlement passes for open transactions and object synchronization.
//! Publish all changes through the event hub.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use driver::{Driver, ObjectQuery, SettleOutcome};
use tokio::sync::Notify;
use wire_types::ObjectSummary;

use crate::events::{Event, EventHub};
use crate::runs::RunRegistry;

/// Interval between settlement passes while transactions remain open.
pub const SETTLE_INTERVAL: Duration = Duration::from_millis(1500);

pub struct Settler {
    driver: Arc<Driver>,
    events: EventHub,
    runs: RunRegistry,
    /// Prevent overlapping background and client-requested settlement passes.
    pass_lock: Mutex<()>,
    wake: Notify,
}

impl Settler {
    pub fn new(driver: Arc<Driver>, events: EventHub, runs: RunRegistry) -> Self {
        Self {
            driver,
            events,
            runs,
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

    /// The pass extended to every object file, then the listing. Blocking.
    pub fn sync_objects(&self, query: Option<&ObjectQuery>) -> Result<Vec<ObjectSummary>> {
        let _pass = self.pass_lock.lock().expect("pass lock poisoned");
        let synced = self.driver.sync_objects(query)?;
        self.publish(&synced.outcome);
        let mut objects = synced.objects;
        crate::runs::fill_held_by(&self.driver, &self.runs, &mut objects);
        Ok(objects)
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
        let mut objects = outcome.objects.clone();
        crate::runs::fill_held_by(&self.driver, &self.runs, &mut objects);
        for object in objects {
            self.events.publish(Event::Object(object));
        }
    }

    /// Runs passes while any transaction is open, at `SETTLE_INTERVAL` and
    /// at once when woken.
    pub async fn run_loop(self: Arc<Self>) {
        loop {
            let settler = self.clone();
            let open = tokio::task::spawn_blocking(move || {
                settler
                    .driver
                    .list_transactions()
                    .map(|records| records.iter().any(|record| record.is_open()))
            })
            .await
            .unwrap_or_else(|err| Err(anyhow!("settle task panicked: {err}")));
            match open {
                Ok(false) => {
                    self.wake.notified().await;
                    continue;
                }
                Ok(true) => {}
                Err(err) => tracing::warn!("could not read transactions: {err:#}"),
            }
            let settler = self.clone();
            match tokio::task::spawn_blocking(move || settler.pass()).await {
                Ok(Ok(outcome)) if !outcome.records.is_empty() => {
                    tracing::info!(changed = outcome.records.len(), "settlement pass");
                }
                Ok(Ok(_)) => {}
                Ok(Err(err)) => tracing::warn!("settlement pass failed: {err:#}"),
                Err(err) => tracing::error!("settlement pass panicked: {err}"),
            }
            tokio::select! {
                _ = tokio::time::sleep(SETTLE_INTERVAL) => {}
                _ = self.wake.notified() => {}
            }
        }
    }
}
