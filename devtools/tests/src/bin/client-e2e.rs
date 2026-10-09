//! Runs one action end to end: proves it, submits it to the relayer and waits until the
//! synchronizer has it.
//!
//! Usage: `client-e2e <action>`, with `RELAYER_URL` and `SYNCHRONIZER_URL` set in this crate's
//! `.env` (see `.env.example`). `<action>` is one of:
//! - `FindLog`: creates one log.
//! - `BurnLogs`: creates the logs BurnLogs spends with one FindLog each, lands them, then burns
//!   them in a single transaction.

use std::{
    env,
    time::{Duration, Instant},
};

use pod2::middleware::Hash;
use sdk::SpendableObject;
use tests::{PreparedTx, Prover, Services, burn_logs_action, find_log_action};

/// The number of inputs the BurnLogs script declares.
const BURN_LOGS_INPUTS: usize = 50;

fn main() {
    env_logger::init();
    let action = env::args()
        .nth(1)
        .expect("usage: client-e2e <FindLog|BurnLogs>");
    let services = Services::from_env("client-e2e");

    println!("building the shrink circuit...");
    let prover = Prover::new();

    match action.as_str() {
        "FindLog" => {
            find_logs(&services, &prover, 1);
        }
        "BurnLogs" => burn_logs(&services, &prover),
        other => panic!("unknown action {other}, expected FindLog or BurnLogs"),
    }
}

/// Proves `count` FindLogs, then lands them all and returns the logs.
fn find_logs(services: &Services, prover: &Prover, count: usize) -> Vec<SpendableObject> {
    let proving_started = Instant::now();
    let prepared: Vec<PreparedTx> = (0..count)
        .map(|index| {
            let started = Instant::now();
            let prepared = prover.prove(
                &find_log_action(),
                services.grounding_witness(&[]),
                Vec::new(),
            );
            println!(
                "proved FindLog {}/{count} in {:.2?}",
                index + 1,
                started.elapsed()
            );
            prepared
        })
        .collect();
    let proving_time = proving_started.elapsed();

    let landing_time = land(services, &prepared);
    println!();
    println!("FindLog x{count} proving time: {proving_time:.2?}");
    println!("FindLog x{count} landing time: {landing_time:.2?}");
    println!();
    prepared.into_iter().flat_map(|tx| tx.outputs).collect()
}

fn burn_logs(services: &Services, prover: &Prover) {
    let logs = find_logs(services, prover, BURN_LOGS_INPUTS);
    let log_commitments: Vec<Hash> = logs.iter().map(|log| log.obj.commitment()).collect();

    let proving_started = Instant::now();
    let prepared = prover.prove(
        &burn_logs_action(),
        services.grounding_witness(&log_commitments),
        logs,
    );
    let proving_time = proving_started.elapsed();
    println!("proved BurnLogs in {proving_time:.2?}");

    let landing_time = land(services, &[prepared]);
    println!();
    println!("BurnLogs proving time: {proving_time:.2?}");
    println!("BurnLogs landing time: {landing_time:.2?}");
}

/// Submits the transactions in order and waits until all of them have landed, returning the
/// time from the first submission until the last one landed.
fn land(services: &Services, txs: &[PreparedTx]) -> Duration {
    let timer = Instant::now();
    let mut submitted: Vec<_> = txs
        .iter()
        .map(|tx| {
            let submitted = services.submit(tx);
            println!("submitted as relayer job {}", submitted.job_id);
            submitted
        })
        .collect();
    services.wait_for_landing(&mut submitted, timer);
    timer.elapsed()
}
