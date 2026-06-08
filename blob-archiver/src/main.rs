use alloy::network::Ethereum;
use alloy::providers::{Provider, RootProvider};
use anyhow::anyhow;
use anyhow::Context;
use anyhow::Result;
use backoff::ExponentialBackoffBuilder;
use eth_clients::beacon::{
    self,
    types::{BlobSidecar, Block, BlockHeader, BlockId, Spec},
    BeaconClient,
};
use std::future::Future;
use std::sync::Arc;
use std::{net::SocketAddr, str::FromStr, time::Duration};
use tokio::sync::watch;
use tokio::task::JoinError;
use tokio::time::sleep;
use tracing::debug;
use tracing::warn;
use tracing::{error, info};

mod config;
mod node;

use config::{load_config, AppConfig};
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

    // {
    //     let node = node.clone();
    //     std::thread::spawn(move || -> Result<_, std::io::Error> {
    //         Runtime::new().map(|rt| {
    //             rt.block_on(async {
    //                 let routes = endpoints::routes(node);
    //                 warp::serve(routes).run(([0, 0, 0, 0], 8001)).await
    //             })
    //         })
    //     });
    // }
    // info!("Started HTTP server");

    let mut slot = node.start_slot()?;
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

        node.process_beacon_block_header(&beacon_block_header)
            .await?;

        if node.config.request_rate != 0 {
            let requests = 5;
            let delay_ms = 1000 * requests / node.config.request_rate;
            sleep(Duration::from_millis(delay_ms)).await;
        }

        slot += 1;
    }
}
