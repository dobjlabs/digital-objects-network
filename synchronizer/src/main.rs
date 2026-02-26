use std::sync::Arc;

use anyhow::Result;
use tokio::sync::watch;
use tokio::task::JoinError;
use tracing::{debug, error, info};

mod api;
mod blob;
mod clients;
mod config;
mod db;
mod gsr;
mod node;
mod proof;
mod state_machine;
mod sync_loop;

use api::run_api_server;
use config::load_config;
use db::Db;
use node::Node;
use proof::ProofParser;
use state_machine::StateMachine;
use sync_loop::run_sync_loop;

#[tokio::main]
async fn main() -> Result<()> {
    // In order to view logs, run `RUST_LOG=info cargo run`
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::filter::EnvFilter::from_default_env())
        .init();

    let cfg = load_config()?;
    debug!(?cfg, "Loaded synchronizer config");

    let db = Db::connect(&cfg.db_path)?;
    let state_machine = Arc::new(StateMachine::new(db, Arc::new(ProofParser::new()?))?);
    let node = Arc::new(Node::new(cfg, Arc::clone(&state_machine)).await?);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let server_task = tokio::spawn(run_api_server(
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
