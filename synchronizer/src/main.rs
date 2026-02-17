use std::{sync::Arc, time::Duration};

use synchronizer::clients::beacon::types::BlockId;

use anyhow::Result;
use tracing::{debug, info};

mod node;
use node::Node;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing. In order to view logs, run `RUST_LOG=info cargo run`
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::filter::EnvFilter::from_default_env())
        .init();

    let node = Arc::new(Node::new().await?);

    let spec = node.beacon_cli.get_spec().await?;
    info!(?spec, "Beacon spec");
    let mut head = node
        .beacon_cli
        .get_block_header(BlockId::Head)
        .await?
        .expect("head is not None");
    info!(?head, "Beacon head");

    let mut slot = head.slot;
    loop {
        debug!("checking slot {}", slot);
        let some_beacon_block_header = if slot <= head.slot {
            node.beacon_cli
                .get_block_header(BlockId::Slot(slot))
                .await?
        } else {
            // TODO: Be more fancy and replace this with a stream from an event subscription to
            // Beacon Headers
            tokio::time::sleep(Duration::from_secs(5)).await;
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

        node.process_beacon_block_header(&beacon_block_header)
            .await?;
        // TODO: read from env
        let request_rate = 15;

        let requests = 5;
        let delay_ms = 1000 * requests / request_rate;
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;

        slot += 1;
    }
}
