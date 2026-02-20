use std::{sync::Arc, time::Duration};

use anyhow::Result;
use futures_util::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use synchronizer::clients::beacon::types::{BlockHeader, BlockId, HeadEventData, Topic};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::node::Node;

const HEAD_CHECK_INTERVAL: Duration = Duration::from_secs(12);

enum NextSlotHeader {
    Shutdown,
    Header(Option<BlockHeader>),
}

pub async fn run_sync_loop(
    node: Arc<Node>,
    mut shutdown_rx: watch::Receiver<bool>,
    sync_delay: Duration,
    initial_start_slot: Option<u32>,
) -> Result<()> {
    let spec = node.beacon_cli.get_spec().await?;
    info!(?spec, "Beacon spec");
    let mut head = node
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
    let mut slot: u32 = start_slot;
    let mut head_events: Option<EventSource> = None;
    loop {
        if *shutdown_rx.borrow() {
            info!("Sync loop shutting down");
            return Ok(());
        }

        debug!("checking slot {}", slot);
        let beacon_block_header =
            match next_slot_header(&node, slot, &mut head, &mut head_events, &mut shutdown_rx)
                .await?
            {
                NextSlotHeader::Shutdown => {
                    info!("Sync loop shutting down");
                    return Ok(());
                }
                NextSlotHeader::Header(some_header) => match some_header {
                    Some(block) => block,
                    None => {
                        info!("slot {} has empty block", slot);
                        node.mark_slot_processed(slot, None).await?;
                        slot += 1;
                        continue;
                    }
                },
            };

        let block_number = node
            .process_beacon_block_header(&beacon_block_header)
            .await?;
        node.mark_slot_processed(slot, block_number).await?;
        if wait_or_shutdown(sync_delay, &mut shutdown_rx).await {
            info!("Sync loop shutting down");
            return Ok(());
        }

        slot += 1;
    }
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

async fn next_slot_header(
    node: &Node,
    slot: u32,
    head: &mut BlockHeader,
    head_events: &mut Option<EventSource>,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> Result<NextSlotHeader> {
    if slot <= head.slot {
        return Ok(NextSlotHeader::Header(
            node.beacon_cli
                .get_block_header(BlockId::Slot(slot))
                .await?,
        ));
    }

    loop {
        if head_events.is_none() {
            let stream = node.beacon_cli.subscribe_to_events(&[Topic::Head])?;
            info!("Subscribed to beacon head events");
            *head_events = Some(stream);
            if let Some(header) = resolve_slot_from_head(node, slot, head).await? {
                return Ok(NextSlotHeader::Header(header));
            }
        }
        let event_source = head_events.as_mut().expect("head events present");

        let next_event = tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    return Ok(NextSlotHeader::Shutdown);
                }
                continue;
            }
            _ = tokio::time::sleep(HEAD_CHECK_INTERVAL) => {
                if let Some(header) = resolve_slot_from_head(node, slot, head).await? {
                    return Ok(NextSlotHeader::Header(header));
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
                    debug!("Ignoring non-head event payload: {}", msg.data);
                    continue;
                };
                if head_event.slot < slot {
                    continue;
                }

                if let Some(header) = resolve_slot_from_head(node, slot, head).await? {
                    return Ok(NextSlotHeader::Header(header));
                }
            }
            Some(Err(err)) => {
                warn!(?err, "Beacon event stream error, reconnecting");
                *head_events = None;
                if wait_or_shutdown(Duration::from_secs(1), shutdown_rx).await {
                    return Ok(NextSlotHeader::Shutdown);
                }
            }
            None => {
                warn!("Beacon event stream ended, reconnecting");
                *head_events = None;
                if wait_or_shutdown(Duration::from_secs(1), shutdown_rx).await {
                    return Ok(NextSlotHeader::Shutdown);
                }
            }
        }
    }
}

async fn resolve_slot_from_head(
    node: &Node,
    slot: u32,
    head: &mut BlockHeader,
) -> Result<Option<Option<BlockHeader>>> {
    *head = node
        .beacon_cli
        .get_block_header(BlockId::Head)
        .await?
        .expect("head is not None");

    if head.slot < slot {
        return Ok(None);
    }
    if head.slot == slot {
        return Ok(Some(Some(head.clone())));
    }

    debug!(
        "head is {}, slot {} was skipped, retrieving...",
        head.slot, slot
    );
    Ok(Some(
        node.beacon_cli
            .get_block_header(BlockId::Slot(slot))
            .await?,
    ))
}
