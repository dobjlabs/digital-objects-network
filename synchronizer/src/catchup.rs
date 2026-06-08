use anyhow::Result;
use futures_util::future::join_all;
use tracing::{info, warn};

use crate::node::Node;
use eth_clients::beacon::types::{Block, BlockHeader};

/// Pre-fetched data for a single slot in a catch-up batch.
pub enum FetchedSlot {
    /// Slot had no block produced (skipped slot on beacon chain).
    Missing { slot: u32 },
    /// Slot had a block; header and full beacon block are available.
    Present {
        slot: u32,
        header: BlockHeader,
        block: Block,
    },
}

/// Fetch beacon headers and full blocks for a batch of slots concurrently.
///
/// Returns results in slot order. Individual slot fetch failures are returned as `Err` entries
/// so the caller can decide how to handle them (e.g. stop the batch at the first failure).
pub async fn fetch_batch(node: &Node, slots: &[u32]) -> Vec<Result<FetchedSlot>> {
    // Phase 1: fetch all headers concurrently.
    let header_futures: Vec<_> = slots
        .iter()
        .map(|&slot| async move {
            let header = node.get_beacon_slot_header_with_retry(slot).await?;
            Ok::<_, anyhow::Error>((slot, header))
        })
        .collect();

    let header_results = join_all(header_futures).await;

    // Phase 2: for present headers, fetch full beacon blocks concurrently.
    // Build futures only for slots that have headers.
    let mut block_futures = Vec::new();
    let mut slot_states: Vec<Option<Result<FetchedSlot>>> = Vec::with_capacity(slots.len());

    for result in &header_results {
        match result {
            Ok((slot, Some(header))) => {
                let slot = *slot;
                let root = header.root;
                let header_clone = header.clone();
                block_futures.push(async move {
                    let block = node.get_beacon_block_by_hash_with_retry(slot, root).await?;
                    Ok::<_, anyhow::Error>((slot, header_clone, block))
                });
                // Placeholder — will be filled from block_futures results.
                slot_states.push(None);
            }
            Ok((slot, None)) => {
                slot_states.push(Some(Ok(FetchedSlot::Missing { slot: *slot })));
            }
            Err(err) => {
                // Propagate the error. We can't move out of the borrowed result, so
                // re-create an error with the same message.
                slot_states.push(Some(Err(anyhow::anyhow!("{err:#}"))));
            }
        }
    }

    let block_results = join_all(block_futures).await;

    // Merge block results back into slot_states in order.
    let mut block_iter = block_results.into_iter();
    for state in slot_states.iter_mut() {
        if state.is_none() {
            // This was a Present slot waiting for its block fetch.
            let block_result = block_iter.next().expect("block futures count matches");
            *state = Some(match block_result {
                Ok((slot, header, block)) => Ok(FetchedSlot::Present {
                    slot,
                    header,
                    block,
                }),
                Err(err) => Err(err),
            });
        }
    }

    let batch_size = slots.len();
    let present_count = slot_states
        .iter()
        .filter(|s| matches!(s, Some(Ok(FetchedSlot::Present { .. }))))
        .count();
    let missing_count = slot_states
        .iter()
        .filter(|s| matches!(s, Some(Ok(FetchedSlot::Missing { .. }))))
        .count();
    let error_count = slot_states
        .iter()
        .filter(|s| matches!(s, Some(Err(_))))
        .count();

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

    slot_states.into_iter().map(|s| s.unwrap()).collect()
}
