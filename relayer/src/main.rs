use std::sync::Arc;

use anyhow::Result;
use tokio::sync::watch;
use tokio::task::JoinError;
use tracing::{error, info};

mod api;
mod config;
mod db;
mod eth;
mod model;
mod time_utils;
mod worker;

use api::run_api_server;
use common::proof::{BlobParser, ProofParser};
use config::load_config;
use db::Db;
use eth::{EthClient, EthGateway};
use worker::{run_worker, WorkerConfig};

/// Boot relayer dependencies, then run API server and background worker together.
#[tokio::main]
async fn main() -> Result<()> {
    common::log_init();

    let cfg = load_config()?;

    let db = Arc::new(Db::connect(&cfg.db_url).await?);
    let parser: Arc<dyn BlobParser> = Arc::new(ProofParser::new()?);
    let eth_client: Arc<dyn EthGateway> = Arc::new(EthClient::new(&cfg).await?);

    let worker_cfg = WorkerConfig {
        max_attempts: cfg.max_attempts,
        retry_initial_secs: cfg.retry_initial_secs,
        retry_max_secs: cfg.retry_max_secs,
        receipt_poll_secs: cfg.receipt_poll_secs,
        receipt_timeout_secs: cfg.receipt_timeout_secs,
        idle_sleep_ms: cfg.worker_idle_sleep_ms,
    };

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let api_task = tokio::spawn(run_api_server(
        Arc::clone(&db),
        Arc::clone(&parser),
        cfg.bind,
        shutdown_rx.clone(),
    ));

    let worker_task = tokio::spawn(run_worker(
        Arc::clone(&db),
        Arc::clone(&eth_client),
        worker_cfg,
        shutdown_rx,
    ));

    let mut api_task = api_task;
    let mut worker_task = worker_task;
    let mut api_finished = false;
    let mut worker_finished = false;

    tokio::select! {
        signal_res = tokio::signal::ctrl_c() => {
            signal_res?;
            info!("Received shutdown signal");
            let _ = shutdown_tx.send(true);
        }
        worker_join = &mut worker_task => {
            handle_task_exit("Worker", worker_join)?;
            let _ = shutdown_tx.send(true);
            worker_finished = true;
        }
        api_join = &mut api_task => {
            handle_task_exit("API server", api_join)?;
            let _ = shutdown_tx.send(true);
            api_finished = true;
        }
    }

    if !worker_finished {
        handle_task_exit("Worker", worker_task.await)?;
    }
    if !api_finished {
        handle_task_exit("API server", api_task.await)?;
    }

    Ok(())
}

/// Normalize spawned task exit handling into a single error/logging path.
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
