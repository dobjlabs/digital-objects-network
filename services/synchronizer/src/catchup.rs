use std::collections::HashMap;

use alloy::primitives::B256;
use anyhow::Result;
use futures_util::future::try_join_all;
use tracing::info;

use crate::node::{Node, SlotContext};
use eth_clients::beacon::types::{BlockAndRoot, BlockHeader};

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
/// the batch makes no further network calls. Returns the slots in order, or the first error.
pub async fn fetch_batch(node: &Node, slots: &[u32]) -> Result<Vec<FetchedSlot>> {
    // Phase 1: fetch the header and full block of every slot concurrently.
    let blocks = try_join_all(slots.iter().map(|&slot| async move {
        let Some(header) = node.get_beacon_slot_header_with_retry(slot).await? else {
            return Ok((slot, None));
        };
        let block = node
            .get_beacon_block_by_hash_with_retry(header.root)
            .await?;
        let block = BlockAndRoot {
            root: header.root,
            block,
        };
        Ok::<_, anyhow::Error>((slot, Some((header, block))))
    }))
    .await?;

    // Phase 2: resolve payloads and blobs concurrently. A block whose parent is in the batch
    // reuses it, so a parent is fetched again only when it precedes the batch.
    let blocks_by_root: HashMap<B256, &BlockAndRoot> = blocks
        .iter()
        .filter_map(|(_, fetched)| fetched.as_ref().map(|(_, block)| (block.root, block)))
        .collect();
    let fetched_slots = try_join_all(blocks.iter().map(|(slot, fetched)| async {
        let Some((header, block)) = fetched else {
            return Ok(FetchedSlot::Missing { slot: *slot });
        };
        let parent_block = blocks_by_root.get(&block.block.parent_root).copied();
        let context = node
            .fetch_slot_context(header, &block.block, parent_block)
            .await?;
        Ok::<_, anyhow::Error>(FetchedSlot::Present {
            slot: *slot,
            header: header.clone(),
            context: Box::new(context),
        })
    }))
    .await?;

    info!(
        batch_size = slots.len(),
        present_count = blocks_by_root.len(),
        first_slot = slots.first().copied().unwrap_or(0),
        last_slot = slots.last().copied().unwrap_or(0),
        "Batch fetch complete"
    );

    Ok(fetched_slots)
}
