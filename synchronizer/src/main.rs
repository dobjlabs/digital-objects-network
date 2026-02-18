use std::{net::SocketAddr, sync::Arc, time::Duration};

use futures_util::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use synchronizer::clients::beacon::types::{BlockHeader, BlockId, HeadEventData, Topic};

use anyhow::Result;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

mod db;
mod endpoints;
mod node;
use db::ensure_database_exists;
use endpoints::run_server;
use node::Node;

const DEFAULT_DATABASE_URL: &str = "postgres://postgres@localhost:5432/synchronizer";
const DEFAULT_HTTP_BIND: &str = "127.0.0.1:3000";
const DEFAULT_SYNC_DELAY_MS: u64 = 333;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing. In order to view logs, run `RUST_LOG=info cargo run`
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::filter::EnvFilter::from_default_env())
        .init();

    let database_url =
        dotenvy::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_string());
    info!("Using configured database URL");
    ensure_database_exists(&database_url).await?;

    let http_bind = dotenvy::var("HTTP_BIND").unwrap_or_else(|_| DEFAULT_HTTP_BIND.to_string());
    let http_bind: SocketAddr = http_bind.parse()?;
    let sync_delay_ms = dotenvy::var("SYNC_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_SYNC_DELAY_MS);

    let node = Arc::new(Node::new(&database_url).await?);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let server_task = tokio::spawn(run_server(
        Arc::clone(&node),
        http_bind,
        shutdown_rx.clone(),
    ));
    let sync_task = tokio::spawn(run_sync_loop(
        Arc::clone(&node),
        shutdown_rx,
        Duration::from_millis(sync_delay_ms),
    ));

    let mut server_task = Some(server_task);
    let mut sync_task = Some(sync_task);

    tokio::select! {
        signal_res = tokio::signal::ctrl_c() => {
            signal_res?;
            info!("Shutdown signal received");
            let _ = shutdown_tx.send(true);
        }
        sync_join = async { sync_task.as_mut().expect("task present").await } => {
            match sync_join {
                Ok(Ok(())) => info!("Sync loop exited"),
                Ok(Err(err)) => {
                    error!(?err, "Sync loop stopped with error");
                    let _ = shutdown_tx.send(true);
                    return Err(err);
                }
                Err(err) => {
                    let join_err = anyhow::anyhow!("sync loop join error: {err}");
                    error!(?join_err, "Sync loop join failed");
                    let _ = shutdown_tx.send(true);
                    return Err(join_err);
                }
            }
            let _ = shutdown_tx.send(true);
            sync_task = None;
        }
        server_join = async { server_task.as_mut().expect("task present").await } => {
            match server_join {
                Ok(Ok(())) => info!("HTTP server exited"),
                Ok(Err(err)) => {
                    error!(?err, "HTTP server stopped with error");
                    let _ = shutdown_tx.send(true);
                    return Err(err);
                }
                Err(err) => {
                    let join_err = anyhow::anyhow!("HTTP server join error: {err}");
                    error!(?join_err, "HTTP server join failed");
                    let _ = shutdown_tx.send(true);
                    return Err(join_err);
                }
            }
            let _ = shutdown_tx.send(true);
            server_task = None;
        }
    }

    if let Some(task) = server_task {
        match task.await {
            Ok(Ok(())) => info!("HTTP server stopped"),
            Ok(Err(err)) => return Err(err),
            Err(err) => return Err(anyhow::anyhow!("HTTP server join error: {err}")),
        }
    }
    if let Some(task) = sync_task {
        match task.await {
            Ok(Ok(())) => info!("Sync loop stopped"),
            Ok(Err(err)) => return Err(err),
            Err(err) => return Err(anyhow::anyhow!("sync loop join error: {err}")),
        }
    }

    Ok(())
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

async fn run_sync_loop(
    node: Arc<Node>,
    mut shutdown_rx: watch::Receiver<bool>,
    sync_delay: Duration,
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
        Some(last_slot) => last_slot.saturating_add(1),
        None => head.slot,
    };
    info!(start_slot, "Starting slot");
    let mut slot = start_slot;
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
                        debug!("slot {} has empty block", slot);
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
        }
        let event_source = head_events.as_mut().expect("head events present");

        let next_event = tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    return Ok(NextSlotHeader::Shutdown);
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

                *head = node
                    .beacon_cli
                    .get_block_header(BlockId::Head)
                    .await?
                    .expect("head is not None");
                if head.slot > slot {
                    debug!(
                        "head is {}, slot {} was skipped, retrieving...",
                        head.slot, slot
                    );
                    return Ok(NextSlotHeader::Header(
                        node.beacon_cli
                            .get_block_header(BlockId::Slot(slot))
                            .await?,
                    ));
                }
                if head.slot == slot {
                    return Ok(NextSlotHeader::Header(Some(head.clone())));
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

enum NextSlotHeader {
    Shutdown,
    Header(Option<BlockHeader>),
}
