use std::sync::Arc;

use anyhow::Result;
use tokio::sync::watch;
use tokio::task::JoinError;
use tracing::{debug, error, info};

mod api;
mod config;
mod db;
mod node;
mod sync_loop;
use api::run_api_server;
use config::load_config;
use node::Node;
use sync_loop::run_sync_loop;

#[tokio::main]
async fn main() -> Result<()> {
    // In order to view logs, run `RUST_LOG=info cargo run`
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::filter::EnvFilter::from_default_env())
        .init();

    let cfg = load_config()?;
    debug!(?cfg, "Loaded config");

    let node: Arc<Node> = Arc::new(Node::new(cfg).await?);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let server_task = tokio::spawn(run_api_server(
        Arc::clone(&node),
        node.config.http_bind,
        shutdown_rx.clone(),
    ));
    let sync_task = tokio::spawn(run_sync_loop(
        Arc::clone(&node),
        shutdown_rx,
        node.config.sync_delay,
        node.config.initial_start_slot,
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
            handle_task_exit("Sync loop", sync_join)?;
            let _ = shutdown_tx.send(true);
            sync_task = None;
        }
        server_join = async { server_task.as_mut().expect("task present").await } => {
            handle_task_exit("HTTP server", server_join)?;
            let _ = shutdown_tx.send(true);
            server_task = None;
        }
    }

    if let Some(task) = server_task {
        handle_task_exit("HTTP server", task.await)?;
    }
    if let Some(task) = sync_task {
        handle_task_exit("Sync loop", task.await)?;
    }

    Ok(())
}

fn handle_task_exit(task_name: &str, join_result: Result<Result<()>, JoinError>) -> Result<()> {
    match join_result {
        Ok(Ok(())) => {
            info!("{task_name} exited");
            Ok(())
        }
        Ok(Err(err)) => {
            error!(?err, "{task_name} stopped with error");
            Err(err)
        }
        Err(err) => {
            let join_err = anyhow::anyhow!("{task_name} join error: {err}");
            error!(?join_err, "{task_name} join failed");
            Err(join_err)
        }
    }
}
