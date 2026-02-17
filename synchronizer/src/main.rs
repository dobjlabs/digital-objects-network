use std::{net::SocketAddr, sync::Arc, time::Duration};

use synchronizer::clients::beacon::types::BlockId;

use anyhow::Result;
use tracing::{debug, error, info};

mod db;
mod endpoints;
mod node;
use db::ensure_database_exists;
use endpoints::run_server;
use node::Node;

const DEFAULT_DATABASE_URL: &str = "postgres://postgres@localhost:5432/synchronizer";
const DEFAULT_HTTP_BIND: &str = "127.0.0.1:3000";

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing. In order to view logs, run `RUST_LOG=info cargo run`
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::filter::EnvFilter::from_default_env())
        .init();

    let database_url =
        dotenvy::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_string());
    info!(%database_url, "Using database URL");
    ensure_database_exists(&database_url).await?;

    let http_bind = dotenvy::var("HTTP_BIND").unwrap_or_else(|_| DEFAULT_HTTP_BIND.to_string());
    let http_bind: SocketAddr = http_bind.parse()?;

    let node = Arc::new(Node::new(&database_url).await?);
    let endpoint_node = Arc::clone(&node);
    tokio::spawn(async move {
        if let Err(err) = run_server(endpoint_node, http_bind).await {
            error!(?err, "State API server exited");
        }
    });

    let spec = node.beacon_cli.get_spec().await?;
    info!(?spec, "Beacon spec");
    let mut head = node
        .beacon_cli
        .get_block_header(BlockId::Head)
        .await?
        .expect("head is not None");
    info!(?head, "Beacon head");

    let start_slot = match node.last_processed_slot().await? {
        Some(last_slot) => last_slot.saturating_add(1),
        None => head.slot,
    };
    info!(start_slot, "Starting slot");
    let mut slot = start_slot;
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
                node.mark_slot_processed(slot, None).await?;
                slot += 1;
                continue;
            }
        };

        let block_number = node
            .process_beacon_block_header(&beacon_block_header)
            .await?;
        node.mark_slot_processed(slot, block_number).await?;
        // TODO: read from env
        let request_rate = 15;

        let requests = 5;
        let delay_ms = 1000 * requests / request_rate;
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;

        slot += 1;
    }
}
