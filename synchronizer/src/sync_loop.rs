use std::{sync::Arc, time::Duration};

use anyhow::Result;
use futures_util::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use synchronizer::clients::beacon::types::{BlockHeader, BlockId, HeadEventData, Topic};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::node::Node;

const HEAD_CHECK_INTERVAL: Duration = Duration::from_secs(12);

enum SlotHeaderState {
    Shutdown,
    Missing,
    Present(BlockHeader),
}

enum HeadCheckResult {
    BehindTarget,
    Missing,
    Present(BlockHeader),
}

struct HeadTracker {
    head: BlockHeader,
    events: Option<EventSource>,
}

struct SyncStart {
    next_slot: u32,
    head_tracker: HeadTracker,
}

pub async fn run_sync_loop(
    node: Arc<Node>,
    mut shutdown_rx: watch::Receiver<bool>,
    sync_delay: Duration,
    initial_start_slot: Option<u32>,
) -> Result<()> {
    let SyncStart {
        mut next_slot,
        mut head_tracker,
    } = initialize_sync(&node, initial_start_slot).await?;

    loop {
        if *shutdown_rx.borrow() {
            info!("Sync loop shutting down");
            return Ok(());
        }

        debug!("checking slot {}", next_slot);
        let beacon_block_header = match head_tracker
            .next_slot_header(&node, next_slot, &mut shutdown_rx)
            .await?
        {
            SlotHeaderState::Shutdown => {
                info!("Sync loop shutting down");
                return Ok(());
            }
            SlotHeaderState::Missing => {
                info!("slot {} has empty block", next_slot);
                node.mark_slot_processed(next_slot, None).await?;
                next_slot += 1;
                continue;
            }
            SlotHeaderState::Present(header) => header,
        };

        let block_number = node
            .process_beacon_block_header(&beacon_block_header)
            .await?;

        node.mark_slot_processed(next_slot, block_number).await?;

        if wait_or_shutdown(sync_delay, &mut shutdown_rx).await {
            info!("Sync loop shutting down");
            return Ok(());
        }

        next_slot += 1;
    }
}

async fn initialize_sync(node: &Node, initial_start_slot: Option<u32>) -> Result<SyncStart> {
    let spec = node.beacon_cli.get_spec().await?;
    info!(?spec, "Beacon spec");

    let head = node
        .beacon_cli
        .get_block_header(BlockId::Head)
        .await?
        .expect("head is not None");
    info!(?head, "Beacon head");

    let start_slot = match node.last_processed_slot().await? {
        Some(last_slot) => last_slot + 1,
        None => initial_start_slot.unwrap_or(head.slot),
    };
    info!(start_slot, head_slot = head.slot, "Starting slot");

    Ok(SyncStart {
        next_slot: start_slot,
        head_tracker: HeadTracker { head, events: None },
    })
}

async fn wait_or_shutdown(duration: Duration, shutdown_rx: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(duration) => false,
        changed = shutdown_rx.changed() => {
            if changed.is_err() {
                return true;
            }
            *shutdown_rx.borrow()
        }
    }
}

impl HeadTracker {
    async fn next_slot_header(
        &mut self,
        node: &Node,
        slot: u32,
        shutdown_rx: &mut watch::Receiver<bool>,
    ) -> Result<SlotHeaderState> {
        if slot <= self.head.slot {
            return Ok(
                match node
                    .beacon_cli
                    .get_block_header(BlockId::Slot(slot))
                    .await?
                {
                    Some(header) => SlotHeaderState::Present(header),
                    None => SlotHeaderState::Missing,
                },
            );
        }

        loop {
            if self.events.is_none() {
                let stream = node.beacon_cli.subscribe_to_events(&[Topic::Head])?;
                info!("Subscribed to beacon head events");
                self.events = Some(stream);

                match self.resolve_slot_from_head(node, slot).await? {
                    HeadCheckResult::BehindTarget => {}
                    HeadCheckResult::Missing => return Ok(SlotHeaderState::Missing),
                    HeadCheckResult::Present(header) => {
                        return Ok(SlotHeaderState::Present(header))
                    }
                }
            }
            let event_source = self.events.as_mut().expect("head events present");

            let next_event = tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        return Ok(SlotHeaderState::Shutdown);
                    }
                    continue;
                }
                _ = tokio::time::sleep(HEAD_CHECK_INTERVAL) => {
                    match self.resolve_slot_from_head(node, slot).await? {
                        HeadCheckResult::BehindTarget => continue,
                        HeadCheckResult::Missing => return Ok(SlotHeaderState::Missing),
                        HeadCheckResult::Present(header) => return Ok(SlotHeaderState::Present(header)),
                    }
                }
                event = event_source.next() => event,
            };

            match next_event {
                Some(Ok(Event::Open)) => {
                    debug!("Beacon head event stream opened");
                }
                Some(Ok(Event::Message(msg))) => {
                    let Ok(head_event) = serde_json::from_str::<HeadEventData>(&msg.data) else {
                        debug!("Ignoring non-head event payload: {}", msg.data);
                        continue;
                    };
                    if head_event.slot < slot {
                        continue;
                    }

                    match self.resolve_slot_from_head(node, slot).await? {
                        HeadCheckResult::BehindTarget => {}
                        HeadCheckResult::Missing => return Ok(SlotHeaderState::Missing),
                        HeadCheckResult::Present(header) => {
                            return Ok(SlotHeaderState::Present(header));
                        }
                    }
                }
                Some(Err(err)) => {
                    warn!(?err, "Beacon event stream error, reconnecting");
                    self.events = None;
                    if wait_or_shutdown(Duration::from_secs(1), shutdown_rx).await {
                        return Ok(SlotHeaderState::Shutdown);
                    }
                }
                None => {
                    warn!("Beacon event stream ended, reconnecting");
                    self.events = None;
                    if wait_or_shutdown(Duration::from_secs(1), shutdown_rx).await {
                        return Ok(SlotHeaderState::Shutdown);
                    }
                }
            }
        }
    }

    async fn resolve_slot_from_head(&mut self, node: &Node, slot: u32) -> Result<HeadCheckResult> {
        self.head = node
            .beacon_cli
            .get_block_header(BlockId::Head)
            .await?
            .expect("head is not None");

        if self.head.slot < slot {
            return Ok(HeadCheckResult::BehindTarget);
        }
        if self.head.slot == slot {
            return Ok(HeadCheckResult::Present(self.head.clone()));
        }

        debug!(
            "head is {}, slot {} was skipped, retrieving...",
            self.head.slot, slot
        );
        Ok(
            match node
                .beacon_cli
                .get_block_header(BlockId::Slot(slot))
                .await?
            {
                Some(header) => HeadCheckResult::Present(header),
                None => HeadCheckResult::Missing,
            },
        )
    }
}
