use std::collections::HashMap;

use alloy::primitives::B256;
use anyhow::{anyhow, Result};
use futures_util::future::join_all;
use tracing::{info, warn};

use crate::node::{Node, SlotContext};
use eth_clients::beacon::types::{Block, BlockHeader};

/// Pre-fetched data for a single slot in a catch-up batch.
pub enum FetchedSlot {
    /// Slot had no block produced (skipped slot on beacon chain).
    Missing { slot: u32 },
    /// Slot had a block; everything needed to derive it has been fetched.
    Present {
        slot: u32,
        header: BlockHeader,
        context: Box<SlotContext>,
    },
}

/// Fetch everything a batch of slots needs to be derived, concurrently, so that processing
/// the batch makes no further network calls.
///
/// Returns results in slot order. Individual slot fetch failures are returned as `Err` entries
/// so the caller can decide how to handle them (e.g. stop the batch at the first failure).
pub async fn fetch_batch(node: &Node, slots: &[u32]) -> Vec<Result<FetchedSlot>> {
    // Phase 1: fetch the header and full block of every slot concurrently.
    let blocks = join_all(slots.iter().map(|&slot| async move {
        let Some(header) = node.get_beacon_slot_header_with_retry(slot).await? else {
            return Ok((slot, None));
        };
        let block = node
            .get_beacon_block_by_hash_with_retry(header.root)
            .await?;
        Ok::<_, anyhow::Error>((slot, Some((header, block))))
    }))
    .await;

    // Phase 2: resolve payloads and blobs concurrently. A block whose parent is in the batch
    // reuses it, so a parent is fetched again only when it precedes the batch.
    let blocks_by_root: HashMap<B256, &Block> = blocks
        .iter()
        .filter_map(|result| match result {
            Ok((_, Some((header, block)))) => Some((header.root, block)),
            _ => None,
        })
        .collect();
    let slot_states: Vec<Result<FetchedSlot>> = join_all(blocks.iter().map(|result| async {
        match result {
            Ok((slot, None)) => Ok(FetchedSlot::Missing { slot: *slot }),
            Ok((slot, Some((header, block)))) => {
                let parent_block = blocks_by_root.get(&block.parent_root).copied();
                let context = node.fetch_slot_context(header, block, parent_block).await?;
                Ok(FetchedSlot::Present {
                    slot: *slot,
                    header: header.clone(),
                    context: Box::new(context),
                })
            }
            // The error cannot be moved out of the borrowed result, so carry its message.
            Err(err) => Err(anyhow!("{err:#}")),
        }
    }))
    .await;

    let batch_size = slots.len();
    let present_count = slot_states
        .iter()
        .filter(|s| matches!(s, Ok(FetchedSlot::Present { .. })))
        .count();
    let missing_count = slot_states
        .iter()
        .filter(|s| matches!(s, Ok(FetchedSlot::Missing { .. })))
        .count();
    let error_count = slot_states.iter().filter(|s| s.is_err()).count();

    info!(
        batch_size,
        present_count,
        missing_count,
        error_count,
        first_slot = slots.first().copied().unwrap_or(0),
        last_slot = slots.last().copied().unwrap_or(0),
        "Batch fetch complete"
    );

    if error_count > 0 {
        warn!(error_count, "Some slots in batch failed to fetch");
    }

    slot_states
}
