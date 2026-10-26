use std::{sync::Arc, time::Duration};

use anyhow::Result;
use eth_clients::{
    beacon::types::{BlockAndRoot, BlockHeader, BlockId, HeadEventData, Topic},
    common::is_node_error,
};
use futures_util::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::catchup::{self, FetchedSlot};
use crate::node::{Node, ProcessedSlot};
const HEAD_CHECK_INTERVAL: Duration = Duration::from_secs(12);
/// Wait before syncing again after the sync loop failed.
const RESTART_DELAY: Duration = Duration::from_secs(10);

enum SlotHeaderState {
    Shutdown,
    Missing,
    Present(BlockHeader),
}

struct HeadTracker {
    head: BlockHeader,
    events: Option<EventSource>,
}

pub(crate) struct SyncStart {
    pub(crate) next_slot: u32,
    pub(crate) head: BlockHeader,
}

enum MissingSlotAction {
    Rewound(u32),
    Applied,
}

/// Sync until shutdown. A node failure delays syncing instead of stopping it: syncing starts
/// over from the committed progress. Any other failure is returned.
pub(crate) async fn run_sync_forever(
    node: Arc<Node>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    loop {
        let result = match initialize_sync(&node).await {
            Ok(sync_start) => {
                run_sync_loop(
                    Arc::clone(&node),
                    shutdown_rx.clone(),
                    node.config.sync_delay,
                    sync_start,
                )
                .await
            }
            Err(err) => Err(err),
        };
        let Err(err) = result else {
            return Ok(());
        };
        if !is_node_error(&err) {
            return Err(err);
        }
        error!(
            ?err,
            "Sync failed; restarting in {} seconds",
            RESTART_DELAY.as_secs()
        );
        if wait_or_shutdown(RESTART_DELAY, &mut shutdown_rx).await {
            info!("Sync loop shutting down");
            return Ok(());
        }
    }
}

/// Process beacon slots in order until shutdown, rewinding when chain history diverges.
async fn run_sync_loop(
    node: Arc<Node>,
    mut shutdown_rx: watch::Receiver<bool>,
    sync_delay: Duration,
    sync_start: SyncStart,
) -> Result<()> {
    let mut next_slot = sync_start.next_slot;
    let mut head_tracker = HeadTracker {
        head: sync_start.head,
        events: None,
    };
    let mut prev_block = None;

    let batch_size = node.config.catchup_batch_size;

    loop {
        if *shutdown_rx.borrow() {
            info!("Sync loop shutting down");
            return Ok(());
        }

        let slots_behind = head_tracker.head.slot.saturating_sub(next_slot);

        // ── Batch catch-up path ──────────────────────────────────────────
        if slots_behind >= batch_size as u32 {
            let batch_end = next_slot + batch_size as u32;
            let slots: Vec<u32> = (next_slot..batch_end).collect();
            info!(
                first_slot = next_slot,
                last_slot = batch_end - 1,
                slots_behind,
                batch_size,
                "Starting batch catch-up"
            );

            let fetched = catchup::fetch_batch(&node, &slots).await?;

            let mut reorg_detected = false;
            for fetched_slot in fetched {
                if *shutdown_rx.borrow() {
                    info!("Sync loop shutting down");
                    return Ok(());
                }

                match fetched_slot {
                    FetchedSlot::Missing { slot } => {
                        match handle_missing_slot(&node, slot).await? {
                            MissingSlotAction::Rewound(rewind_slot) => {
                                next_slot = rewind_slot;
                                reorg_detected = true;
                                break;
                            }
                            MissingSlotAction::Applied => {
                                next_slot = slot + 1;
                            }
                        }
                    }
                    FetchedSlot::Present {
                        slot,
                        header,
                        context,
                    } => {
                        if let Some(rewind_slot) =
                            handle_reorgs_for_present_slot(&node, slot, &header).await?
                        {
                            next_slot = rewind_slot;
                            reorg_detected = true;
                            break;
                        }

                        let processed = node.derive_from_context(*context).await?;
                        node.commit_slot(&processed).await?;
                        next_slot = slot + 1;
                    }
                }
            }

            if reorg_detected {
                // Re-fetch beacon head before restarting: the chain view may have changed.
                head_tracker.head = node.get_beacon_head_header().await?;
            }

            continue;
        }

        // ── Single-slot path (at or near head) ──────────────────────────
        debug!(slot = next_slot, "Checking slot");
        // Resolve the target slot against beacon head; may return:
        // - Missing: empty slot
        // - Present: block header for this slot
        // - Shutdown: graceful stop
        let beacon_block_header = match head_tracker
            .next_slot_header(&node, next_slot, &mut shutdown_rx)
            .await?
        {
            SlotHeaderState::Shutdown => {
                info!("Sync loop shutting down");
                return Ok(());
            }
            SlotHeaderState::Missing => {
                match handle_missing_slot(&node, next_slot).await? {
                    MissingSlotAction::Rewound(rewind_slot) => {
                        next_slot = rewind_slot;
                    }
                    MissingSlotAction::Applied => {
                        next_slot += 1;
                    }
                }
                continue;
            }
            SlotHeaderState::Present(header) => header,
        };

        // For present slots, centralize all "did chain history diverge?" checks
        // before any write side effects.
        if let Some(rewind_slot) =
            handle_reorgs_for_present_slot(&node, next_slot, &beacon_block_header).await?
        {
            next_slot = rewind_slot;
            continue;
        }

        let beacon_block = node.get_beacon_block(beacon_block_header.root).await?;
        let context = node
            .fetch_slot_context(&beacon_block_header, &beacon_block, prev_block.as_ref())
            .await?;
        prev_block = Some(BlockAndRoot {
            root: beacon_block_header.root,
            block: beacon_block,
        });
        let processed = node.derive_from_context(context).await?;
        node.commit_slot(&processed).await?;

        if wait_or_shutdown(sync_delay, &mut shutdown_rx).await {
            info!("Sync loop shutting down");
            return Ok(());
        }

        next_slot += 1;
    }
}

/// Handle a slot that currently has no block header.
///
/// Returns whether the loop should continue from a rewind slot or from the next slot.
async fn handle_missing_slot(node: &Node, slot: u32) -> Result<MissingSlotAction> {
    // A previously committed slot becoming empty implies chain history changed.
    if node.state_log.slot_root(slot).await?.is_some() {
        warn!(
            slot,
            "Detected reorg: slot was previously present but is now missing; rewinding"
        );
        return Ok(MissingSlotAction::Rewound(
            rewind_for_reorg(node, slot).await?,
        ));
    }

    // Skipped slots are valid on beacon; commit one anyway so the slot history stays contiguous.
    info!(slot, "No block produced for slot");
    node.commit_slot(&ProcessedSlot {
        slot,
        block_root: None,
        derived: None,
    })
    .await?;
    Ok(MissingSlotAction::Applied)
}

/// Run all present-slot reorg checks.
///
/// Returns `Some(rewind_slot)` when chain-consistency checks fail, else `None`.
async fn handle_reorgs_for_present_slot(
    node: &Node,
    slot: u32,
    beacon_block_header: &BlockHeader,
) -> Result<Option<u32>> {
    let last_processed_slot = node.state_log.last_processed_slot().await?;
    let stored_root_for_slot = node.state_log.slot_root(slot).await?;
    if last_processed_slot.is_some_and(|last| last >= slot) && stored_root_for_slot.is_none() {
        // A previously empty slot becoming non-empty implies chain history changed.
        warn!(
            slot,
            "Detected reorg: slot was previously empty but now has a block; rewinding"
        );
        return Ok(Some(rewind_for_reorg(node, slot).await?));
    }

    if let Some(stored_root) = stored_root_for_slot {
        // Same slot number with a different block root is a reorg.
        if stored_root != beacon_block_header.root {
            warn!(
                slot,
                stored_root = ?stored_root,
                fetched_root = ?beacon_block_header.root,
                "Detected reorg: block root changed for slot; rewinding"
            );
            return Ok(Some(rewind_for_reorg(node, slot).await?));
        }
    }

    if let Some(prev_root) = node.state_log.slot_root(slot - 1).await? {
        if beacon_block_header.parent_root != prev_root {
            warn!(
                slot,
                expected_parent = ?prev_root,
                actual_parent = ?beacon_block_header.parent_root,
                "Detected reorg: parent linkage mismatch; rewinding"
            );
            return Ok(Some(rewind_for_reorg(node, slot).await?));
        }
    }

    Ok(None)
}

/// Rewind to the first slot after the last common ancestor and resume from there.
async fn rewind_for_reorg(node: &Node, current_slot: u32) -> Result<u32> {
    let rewind_start = find_divergence_slot(node, current_slot).await?;
    let keep_slot = rewind_start - 1;
    node.state_log.rollback_to_slot(keep_slot).await?;
    info!(
        current_slot,
        rewind_start, keep_slot, "Rewound state to divergence boundary"
    );
    Ok(rewind_start)
}

/// Find the first slot to re-derive from: the one after the last block both the stored and the
/// live chain have. The walk stops at `INIT_START_SLOT`, which is finalized.
async fn find_divergence_slot(node: &Node, current_slot: u32) -> Result<u32> {
    let init_start_slot = node.config.init_start_slot;
    let mut slot = current_slot;
    while slot > init_start_slot {
        let prev_slot = slot - 1;
        let stored_root = node.state_log.slot_root(prev_slot).await?;
        let live_root = node
            .beacon_cli
            .get_block_header(BlockId::Slot(prev_slot))
            .await?
            .map(|header| header.root);
        if stored_root.is_some() && stored_root == live_root {
            return Ok(slot);
        }
        slot = prev_slot;
    }
    warn!(
        current_slot,
        init_start_slot, "No common ancestor after INIT_START_SLOT; re-deriving from it"
    );
    Ok(init_start_slot)
}

async fn initialize_sync(node: &Node) -> Result<SyncStart> {
    let spec = node.beacon_cli.get_spec().await?;
    info!(?spec, "Loaded beacon spec");

    let head = node.get_beacon_head_header().await?;
    info!(head_slot = head.slot, head_root = ?head.root, "Fetched initial beacon head");

    let start_slot = match node.state_log.last_processed_slot().await? {
        Some(last_processed_slot) => last_processed_slot + 1,
        None => {
            let init_start_slot = node.config.init_start_slot;
            node.validate_genesis_slot(init_start_slot).await?;
            init_start_slot
        }
    };

    info!(
        start_slot,
        head_slot = head.slot,
        "Initialized sync start slot"
    );

    Ok(SyncStart {
        next_slot: start_slot,
        head,
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
        if slot < self.head.slot {
            info!(
                next_slot = slot,
                head_slot = self.head.slot,
                slots_behind = self.head.slot - slot,
                "Catching up to beacon head"
            );
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
                if let Some(state) = self.resolve_slot_from_head(node, slot).await? {
                    return Ok(state);
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
                    if let Some(state) = self.resolve_slot_from_head(node, slot).await? {
                        return Ok(state);
                    }
                    continue;
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

                    // Events are hints; re-read the beacon head before deciding.
                    if let Some(state) = self.resolve_slot_from_head(node, slot).await? {
                        return Ok(state);
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

    /// Refresh the beacon head and resolve `slot` against it; `None` while the head has not
    /// reached `slot` yet.
    async fn resolve_slot_from_head(
        &mut self,
        node: &Node,
        slot: u32,
    ) -> Result<Option<SlotHeaderState>> {
        self.head = node.get_beacon_head_header().await?;

        if self.head.slot < slot {
            return Ok(None);
        }
        if self.head.slot == slot {
            return Ok(Some(SlotHeaderState::Present(self.head.clone())));
        }

        // Head passed target: explicit target lookup distinguishes produced vs skipped slot.
        debug!(
            head_slot = self.head.slot,
            target_slot = slot,
            "Target slot behind head; fetching explicit slot header"
        );
        Ok(Some(
            match node
                .beacon_cli
                .get_block_header(BlockId::Slot(slot))
                .await?
            {
                Some(header) => SlotHeaderState::Present(header),
                None => SlotHeaderState::Missing,
            },
        ))
    }
}
