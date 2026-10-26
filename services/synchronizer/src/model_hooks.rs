use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::{mpsc, Semaphore};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Flushed,
    SlotInserted,
    IndexInserted,
    ReadSnapshot,
}

pub(crate) struct Gate {
    events: mpsc::UnboundedSender<Stage>,
    permits: Semaphore,
}

impl Gate {
    pub(crate) fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<Stage>) {
        let (events, receiver) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                events,
                permits: Semaphore::new(0),
            }),
            receiver,
        )
    }

    pub(crate) async fn pause(&self, stage: Stage) {
        self.events
            .send(stage)
            .expect("model event receiver closed");
        self.permits
            .acquire()
            .await
            .expect("model gate closed")
            .forget();
    }

    pub(crate) fn proceed(&self) {
        self.permits.add_permits(1);
    }
}

pub(crate) async fn next(receiver: &mut mpsc::UnboundedReceiver<Stage>) -> Result<Stage> {
    tokio::time::timeout(std::time::Duration::from_secs(10), receiver.recv())
        .await
        .context("publication checkpoint timed out")?
        .context("publication task stopped")
}
