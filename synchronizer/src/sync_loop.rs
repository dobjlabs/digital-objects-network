use std::{sync::Arc, time::Duration};

use crate::clients::beacon::types::{BlockHeader, BlockId, HeadEventData, Topic};
use anyhow::Result;
use futures_util::StreamExt;
use reqwest_eventsource::{Event, EventSource};
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

enum AdvanceDecision {
    ContinueWaiting,
    Return(SlotHeaderState),
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

        debug!(slot = next_slot, "Checking slot");
        let beacon_block_header = match head_tracker
            .next_slot_header(&node, next_slot, &mut shutdown_rx)
            .await?
        {
            SlotHeaderState::Shutdown => {
                info!("Sync loop shutting down");
                return Ok(());
            }
            SlotHeaderState::Missing => {
                // A previously canonical slot becoming empty implies canonical history changed.
                if node.slot_root(next_slot)?.is_some() {
                    warn!(
                        slot = next_slot,
                        "Detected reorg: slot was previously canonical but is now missing; rewinding"
                    );
                    next_slot = rewind_for_reorg(&node, next_slot).await?;
                    continue;
                }

                // Empty slots are valid on beacon; persist progress so sync stays in-order.
                info!(slot = next_slot, "No block produced for slot");
                node.set_slot_root(next_slot, None)?;
                node.mark_slot_processed(next_slot, None)?;
                next_slot += 1;
                continue;
            }
            SlotHeaderState::Present(header) => header,
        };

        let last_processed_slot = node.last_processed_slot()?;
        let stored_root_for_slot = node.slot_root(next_slot)?;
        if last_processed_slot.is_some_and(|last_slot| last_slot >= next_slot)
            && stored_root_for_slot.is_none()
        {
            // A previously empty canonical slot becoming non-empty implies canonical history changed.
            warn!(
                slot = next_slot,
                "Detected reorg: slot was previously empty but now has a block; rewinding"
            );
            next_slot = rewind_for_reorg(&node, next_slot).await?;
            continue;
        }

        if let Some(stored_root) = stored_root_for_slot {
            // Same slot number with a different block root is a canonical reorg.
            if stored_root != beacon_block_header.root {
                warn!(
                    slot = next_slot,
                    stored_root = ?stored_root,
                    fetched_root = ?beacon_block_header.root,
                    "Detected reorg: canonical root changed for slot; rewinding"
                );
                next_slot = rewind_for_reorg(&node, next_slot).await?;
                continue;
            }
        }
        if let Some(prev_slot) = next_slot.checked_sub(1) {
            if let Some(prev_root) = node.slot_root(prev_slot)? {
                // Parent mismatch means our local chain view diverged from current canonical chain.
                if beacon_block_header.parent_root != prev_root {
                    warn!(
                        slot = next_slot,
                        expected_parent = ?prev_root,
                        actual_parent = ?beacon_block_header.parent_root,
                        "Detected reorg: parent linkage mismatch; rewinding"
                    );
                    next_slot = rewind_for_reorg(&node, next_slot).await?;
                    continue;
                }
            }
        }

        let block_number = node
            .process_beacon_block_header(&beacon_block_header)
            .await?;
        node.set_slot_root(next_slot, Some(beacon_block_header.root))?;
        node.mark_slot_processed(next_slot, block_number)?;

        if wait_or_shutdown(sync_delay, &mut shutdown_rx).await {
            info!("Sync loop shutting down");
            return Ok(());
        }

        next_slot += 1;
    }
}

async fn rewind_for_reorg(node: &Node, current_slot: u32) -> Result<u32> {
    // Rewind to the first slot after the last matching ancestor, then replay forward.
    let rewind_start = find_divergence_slot(node, current_slot).await?;
    let keep_slot = rewind_start.checked_sub(1);
    node.rollback_to_slot(keep_slot)?;
    info!(
        current_slot,
        rewind_start, keep_slot, "Rewound state to divergence boundary"
    );
    Ok(rewind_start)
}

async fn find_divergence_slot(node: &Node, current_slot: u32) -> Result<u32> {
    let mut slot = current_slot;
    while let Some(prev_slot) = slot.checked_sub(1) {
        // Walk backward until stored and live roots match (last common ancestor).
        let stored_root = node.slot_root(prev_slot)?;
        let live_root = node
            .beacon_cli
            .get_block_header(BlockId::Slot(prev_slot))
            .await?
            .map(|header| header.root);
        if stored_root == live_root {
            return Ok(prev_slot + 1);
        }
        slot = prev_slot;
    }
    Ok(0)
}

async fn initialize_sync(node: &Node, initial_start_slot: Option<u32>) -> Result<SyncStart> {
    let spec = node.beacon_cli.get_spec().await?;
    info!(?spec, "Loaded beacon spec");

    let head = node
        .beacon_cli
        .get_block_header(BlockId::Head)
        .await?
        .expect("head is not None");
    info!(head_slot = head.slot, head_root = ?head.root, "Fetched initial beacon head");

    let start_slot = match node.last_processed_slot()? {
        Some(last_slot) => last_slot + 1,
        None => initial_start_slot.unwrap_or(head.slot),
    };
    info!(start_slot, head_slot = head.slot, "Initialized sync cursor");

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
                info!(target_slot = slot, "Subscribed to beacon head events");
                self.events = Some(stream);

                // Re-check immediately after subscribe to close subscribe-vs-head race windows.
                match Self::decide_after_head_check(self.resolve_slot_from_head(node, slot).await?)
                {
                    AdvanceDecision::ContinueWaiting => {}
                    AdvanceDecision::Return(state) => return Ok(state),
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
                    // Polling fallback keeps liveness if the SSE stream is stale but not closed.
                    match Self::decide_after_head_check(self.resolve_slot_from_head(node, slot).await?) {
                        AdvanceDecision::ContinueWaiting => continue,
                        AdvanceDecision::Return(state) => return Ok(state),
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
                        debug!(payload = %msg.data, "Ignoring unexpected beacon event payload");
                        continue;
                    };
                    if head_event.slot < slot {
                        continue;
                    }

                    // Events are hints; re-read canonical head/slot before deciding.
                    match Self::decide_after_head_check(
                        self.resolve_slot_from_head(node, slot).await?,
                    ) {
                        AdvanceDecision::ContinueWaiting => {}
                        AdvanceDecision::Return(state) => return Ok(state),
                    }
                }
                Some(Err(err)) => {
                    warn!(
                        ?err,
                        target_slot = slot,
                        "Beacon event stream error; reconnecting"
                    );
                    self.events = None;
                    if wait_or_shutdown(Duration::from_secs(1), shutdown_rx).await {
                        return Ok(SlotHeaderState::Shutdown);
                    }
                }
                None => {
                    warn!(
                        target_slot = slot,
                        "Beacon event stream ended; reconnecting"
                    );
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

        // Head passed target: explicit target lookup distinguishes produced vs skipped slot.
        debug!(
            head_slot = self.head.slot,
            target_slot = slot,
            "Target slot behind head; fetching explicit slot header"
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

    fn decide_after_head_check(result: HeadCheckResult) -> AdvanceDecision {
        match result {
            HeadCheckResult::BehindTarget => AdvanceDecision::ContinueWaiting,
            HeadCheckResult::Missing => AdvanceDecision::Return(SlotHeaderState::Missing),
            HeadCheckResult::Present(header) => {
                AdvanceDecision::Return(SlotHeaderState::Present(header))
            }
        }
    }
}
