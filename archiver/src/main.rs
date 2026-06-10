use anyhow::Result;
use eth_clients::beacon::types::BlockId;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::sleep;
use tracing::debug;
use tracing::info;

mod api;
mod config;
mod node;

use api::run_api_server;
use config::load_config;
use node::Node;

#[tokio::main]
async fn main() -> Result<()> {
    common::log_init();

    let config = load_config()?;

    let node = Arc::new(Node::new(config).await?);

    let spec = node.beacon_cli.get_spec().await?;
    info!(?spec, "Beacon spec");
    let mut head = node
        .beacon_cli
        .get_block_header(BlockId::Head)
        .await?
        .expect("head is not None");
    info!(?head, "Beacon head");

    let last_header = Arc::new(RwLock::new(None));
    let api_state = api::ApiState {
        config: Arc::new(api::Config {
            filter_address: node.config.filter_address,
        }),
        store: Arc::new(node.store.clone()),
        header: last_header.clone(),
    };
    tokio::spawn(run_api_server(api_state, node.config.http_bind));

    let mut prev_beacon_block_header = node.store.last_header()?;
    let mut slot = prev_beacon_block_header
        .as_ref()
        .map(|h| h.slot + 1)
        .unwrap_or(node.config.init_start_slot);
    loop {
        debug!("checking slot {}", slot);
        let some_beacon_block_header = if slot <= head.slot {
            node.beacon_cli
                .get_block_header(BlockId::Slot(slot))
                .await?
        } else {
            // TODO: Be more fancy and replace this with a stream from an event subscription to
            // Beacon Headers
            sleep(Duration::from_secs(5)).await;
            loop {
                head = node
                    .beacon_cli
                    .get_block_header(BlockId::Head)
                    .await?
                    .expect("head is not None");
                if head.slot > slot {
                    debug!(
                        "head is {}, slot {} was skipped, retrieving...",
                        head.slot, slot
                    );
                    break node
                        .beacon_cli
                        .get_block_header(BlockId::Slot(slot))
                        .await?;
                } else if head.slot == slot {
                    break Some(head.clone());
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        };
        let beacon_block_header = match some_beacon_block_header {
            Some(block) => block,
            None => {
                debug!("slot {} has empty block", slot);
                slot += 1;
                continue;
            }
        };
        if let Some(prev) = &prev_beacon_block_header {
            if beacon_block_header.parent_root != prev.root {
                info!(
                    "reorg: slot {} ({}) has different parent than us",
                    beacon_block_header.slot, beacon_block_header.root
                );
                node.store.delete_block_data(&node.store.slot_dir(slot))?;
                prev_beacon_block_header = node.store.last_header()?;
                slot = prev_beacon_block_header
                    .as_ref()
                    .map(|h| h.slot + 1)
                    .unwrap_or(node.config.init_start_slot);
                *last_header.write().await = prev_beacon_block_header.clone();
                continue;
            }
        }

        node.process_beacon_block_header(&beacon_block_header)
            .await?;
        prev_beacon_block_header = Some(beacon_block_header);
        *last_header.write().await = prev_beacon_block_header.clone();

        slot += 1;
    }
}
