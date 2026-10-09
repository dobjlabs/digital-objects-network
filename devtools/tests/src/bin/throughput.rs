//! Proves `n` FindLog transactions up front, then measures how long the relayer and the
//! synchronizer take to land all of them.
//!
//! Usage: `throughput <n>`, with `RELAYER_URL` and `SYNCHRONIZER_URL` set in this crate's
//! `.env` (see `.env.example`).

use std::{
    env,
    time::{Duration, Instant},
};

use tests::{PreparedTx, Prover, Services, SubmittedTx, find_log_action};

fn main() {
    env_logger::init();
    let services = Services::from_env("throughput");
    let num_txs: usize = env::args()
        .nth(1)
        .expect("usage: throughput <n>")
        .parse()
        .unwrap();

    let action = find_log_action();
    println!("building the shrink circuit...");
    let prover = Prover::new();

    let prepared: Vec<PreparedTx> = (0..num_txs)
        .map(|index| {
            let started = Instant::now();
            let prepared = prover.prove(&action, services.grounding_witness(&[]), Vec::new());
            println!(
                "proved tx {}/{num_txs} in {:.1?}",
                index + 1,
                started.elapsed()
            );
            prepared
        })
        .collect();

    let timer = Instant::now();
    let mut submitted: Vec<SubmittedTx> = prepared
        .iter()
        .enumerate()
        .map(|(index, tx)| {
            let submitted = services.submit(tx);
            println!(
                "submitted tx {}/{num_txs} as relayer job {}",
                index + 1,
                submitted.job_id
            );
            submitted
        })
        .collect();
    let submission_time = timer.elapsed();

    services.wait_for_landing(&mut submitted, timer);
    let total_time = timer.elapsed();

    println!();
    println!("transactions:     {num_txs}");
    println!("submission time:  {submission_time:.2?}");
    println!("time to land all: {total_time:.2?}");
    println!(
        "throughput:       {:.3} tx/s",
        num_txs as f64 / total_time.as_secs_f64()
    );
    let mut landing_times: Vec<Duration> = submitted
        .iter()
        .map(|tx| tx.landed_after.unwrap())
        .collect();
    landing_times.sort();
    println!("first landed at:  {:.2?}", landing_times[0]);
    println!(
        "median landed at: {:.2?}",
        landing_times[landing_times.len() / 2]
    );
}
