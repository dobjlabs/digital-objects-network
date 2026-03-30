use std::{sync::Arc, time::Duration};

use crate::clients::beacon::types::{BlockHeader, BlockId, HeadEventData, Topic};
use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::node::{Node, ProcessedSlot};
use crate::sync_db::CommittedSlotRecord;

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

pub(crate) struct SyncStart {
    pub(crate) next_slot: u32,
    pub(crate) head: BlockHeader,
}

enum MissingSlotAction {
    Rewound(u32),
    Applied,
}

/// Process beacon slots in order until shutdown, rewinding when canonical history diverges.
pub async fn run_sync_loop(
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

    loop {
        if *shutdown_rx.borrow() {
            info!("Sync loop shutting down");
            return Ok(());
        }

        debug!(slot = next_slot, "Checking slot");
        // Resolve the target slot against beacon head; may return:
        // - Missing: canonical empty slot
        // - Present: canonical block header for this slot
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

        // For present slots, centralize all "did canonical history diverge?" checks
        // before any write side effects.
        if let Some(rewind_slot) =
            handle_reorgs_for_present_slot(&node, next_slot, &beacon_block_header).await?
        {
            next_slot = rewind_slot;
            continue;
        }

        let processed = node.derive_slot_update(&beacon_block_header).await?;
        node.commit_slot(&processed).await?;

        if wait_or_shutdown(sync_delay, &mut shutdown_rx).await {
            info!("Sync loop shutting down");
            return Ok(());
        }

        next_slot += 1;
    }
}

/// Handle a slot that currently has no canonical block header.
///
/// Returns whether the loop should continue from a rewind slot or from the next slot.
async fn handle_missing_slot(node: &Node, slot: u32) -> Result<MissingSlotAction> {
    // A previously canonical slot becoming empty implies canonical history changed.
    if node.slot_root(slot).await?.is_some() {
        warn!(
            slot,
            "Detected reorg: slot was previously canonical but is now missing; rewinding"
        );
        return Ok(MissingSlotAction::Rewound(
            rewind_for_reorg(node, slot).await?,
        ));
    }

    // Empty slots are valid on beacon; commit an empty processed slot so canonical slot history
    // stays contiguous.
    let head = node.current_head().await?;
    let processed = ProcessedSlot::empty(
        slot,
        Default::default(),
        Default::default(),
        node.state_machine.noop_head(head),
    );

    info!(slot, "No block produced for slot");
    node.commit_slot(&processed).await?;
    Ok(MissingSlotAction::Applied)
}

/// Run all present-slot reorg checks.
///
/// Returns `Some(rewind_slot)` when canonical-consistency checks fail, else `None`.
async fn handle_reorgs_for_present_slot(
    node: &Node,
    slot: u32,
    beacon_block_header: &BlockHeader,
) -> Result<Option<u32>> {
    let last_processed_slot = node.last_processed_slot().await?;
    let stored_root_for_slot = node.slot_root(slot).await?;
    if last_processed_slot >= slot && stored_root_for_slot.is_none() {
        // A previously empty canonical slot becoming non-empty implies canonical history changed.
        warn!(
            slot,
            "Detected reorg: slot was previously empty but now has a block; rewinding"
        );
        return Ok(Some(rewind_for_reorg(node, slot).await?));
    }

    if let Some(stored_root) = stored_root_for_slot {
        // Same slot number with a different block root is a canonical reorg.
        if stored_root != beacon_block_header.root {
            warn!(
                slot,
                stored_root = ?stored_root,
                fetched_root = ?beacon_block_header.root,
                "Detected reorg: canonical root changed for slot; rewinding"
            );
            return Ok(Some(rewind_for_reorg(node, slot).await?));
        }
    }

    if let Some(prev_slot) = slot.checked_sub(1) {
        if let Some(prev_root) = node.slot_root(prev_slot).await? {
            // Parent mismatch means our local chain view diverged from current canonical chain.
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
    }

    Ok(None)
}

/// Rewind to the first slot after the last common ancestor and resume from there.
async fn rewind_for_reorg(node: &Node, current_slot: u32) -> Result<u32> {
    let rewind_start = find_divergence_slot(node, current_slot).await?;
    let keep_slot = rewind_start - 1;
    node.rollback_to_slot(keep_slot).await?;
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
        let stored_root = node.slot_root(prev_slot).await?;
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
    Err(anyhow!(
        "No common ancestor found while handling reorg at slot {current_slot};"
    ))
}

pub(crate) async fn initialize_sync(
    node: &Node,
    initial_start_slot: Option<u32>,
) -> Result<SyncStart> {
    let spec = node.beacon_cli.get_spec().await?;
    info!(?spec, "Loaded beacon spec");

    let head = node
        .beacon_cli
        .get_block_header(BlockId::Head)
        .await?
        .expect("head is not None");
    info!(head_slot = head.slot, head_root = ?head.root, "Fetched initial beacon head");

    let bootstrap_start_slot = initial_start_slot.unwrap_or(head.slot);
    let bootstrap_slot = load_bootstrap_slot_record(node, &head, bootstrap_start_slot).await?;

    let start_slot = node
        .sync_db
        .ensure_cursor_and_get_start_slot(bootstrap_slot)
        .await?;

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

async fn load_bootstrap_slot_record(
    node: &Node,
    current_head: &BlockHeader,
    start_slot: u32,
) -> Result<CommittedSlotRecord> {
    let bootstrap_slot = start_slot.checked_sub(1).ok_or_else(|| {
        anyhow!("bootstrap start slot must be > 0 to insert initial canonical row")
    })?;

    if bootstrap_slot > current_head.slot {
        return Err(anyhow!(
            "INITIAL_START_SLOT {start_slot} is ahead of current beacon head {}; cannot bootstrap slot {}",
            current_head.slot,
            bootstrap_slot
        ));
    }

    let Some(header) = node
        .beacon_cli
        .get_block_header(BlockId::Slot(bootstrap_slot))
        .await?
    else {
        return Ok(CommittedSlotRecord {
            slot: bootstrap_slot,
            block_root: None,
            parent_root: None,
            block_number: None,
            current_gsr: None,
            is_empty: true,
        });
    };

    let block = node
        .beacon_cli
        .get_block(BlockId::Hash(header.root))
        .await?
        .ok_or_else(|| {
            anyhow!(
                "Beacon header exists for bootstrap slot {bootstrap_slot}, but full beacon block {} was not found",
                header.root
            )
        })?;

    let execution_payload = block.execution_payload.as_ref().ok_or_else(|| {
        anyhow!(
            "Beacon block {} for bootstrap slot {bootstrap_slot} had no execution payload",
            header.root
        )
    })?;

    Ok(CommittedSlotRecord {
        slot: bootstrap_slot,
        block_root: Some(header.root),
        parent_root: Some(block.parent_root),
        block_number: Some(execution_payload.block_number),
        current_gsr: None,
        is_empty: false,
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
