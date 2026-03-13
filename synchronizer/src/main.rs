use std::sync::Arc;

use anyhow::Result;
use tokio::sync::watch;
use tokio::task::JoinError;
use tracing::{debug, error, info};

mod api;
mod app_db;
mod clients;
mod config;
mod node;
mod state_machine;
mod sync_db;
mod sync_loop;

use api::run_api_server;
use app_db::AppDb;
use common::proof::ProofParser;
use config::load_config;
use node::Node;
use state_machine::StateMachine;
use sync_db::SyncDb;
use sync_loop::run_sync_loop;

#[tokio::main]
async fn main() -> Result<()> {
    common::log_init();

    let cfg = load_config()?;
    debug!(?cfg, "Loaded synchronizer config");

    let app_db = AppDb::connect(&cfg.app_state_db_path)?;
    let sync_db = Arc::new(SyncDb::connect(&cfg.sync_metadata_db_url).await?);
    let state_machine = Arc::new(StateMachine::new(app_db, Arc::new(ProofParser::new()?))?);
    let node = Arc::new(Node::new(cfg, Arc::clone(&state_machine), Arc::clone(&sync_db)).await?);
    node.recover_pending().await?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let server_task = tokio::spawn(run_api_server(
        Arc::clone(&sync_db),
        Arc::clone(&state_machine),
        node.config.http_bind,
        shutdown_rx.clone(),
    ));
    let sync_task = tokio::spawn(run_sync_loop(
        Arc::clone(&node),
        shutdown_rx,
        node.config.sync_delay,
        node.config.initial_start_slot,
    ));

    let mut server_task = server_task;
    let mut sync_task = sync_task;
    let mut server_finished = false;
    let mut sync_finished = false;

    tokio::select! {
        signal_res = tokio::signal::ctrl_c() => {
            signal_res?;
            info!("Received shutdown signal");
            let _ = shutdown_tx.send(true);
        }
        sync_join = &mut sync_task => {
            handle_task_exit("Sync loop", sync_join)?;
            let _ = shutdown_tx.send(true);
            sync_finished = true;
        }
        server_join = &mut server_task => {
            handle_task_exit("HTTP server", server_join)?;
            let _ = shutdown_tx.send(true);
            server_finished = true;
        }
    }

    if !server_finished {
        handle_task_exit("HTTP server", server_task.await)?;
    }
    if !sync_finished {
        handle_task_exit("Sync loop", sync_task.await)?;
    }

    Ok(())
}

fn handle_task_exit(task_name: &str, join_result: Result<Result<()>, JoinError>) -> Result<()> {
    match join_result {
        Ok(Ok(())) => {
            info!(task = task_name, "Task exited cleanly");
            Ok(())
        }
        Ok(Err(err)) => {
            error!(task = task_name, ?err, "Task stopped with error");
            Err(err)
        }
        Err(err) => {
            let join_err = anyhow::anyhow!("{task_name} join error: {err}");
            error!(task = task_name, ?join_err, "Task join failed");
            Err(join_err)
        }
    }
}
