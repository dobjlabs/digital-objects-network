//! Runs one FindLog end to end: proves it, submits it to the relayer and waits until the
//! synchronizer has it.
//!
//! Usage: `client-e2e`, with `RELAYER_URL` and `SYNCHRONIZER_URL` set in this crate's `.env`
//! (see `.env.example`).

use std::time::Instant;

use tests::{Prover, Services, find_log_action};

fn main() {
    env_logger::init();
    let services = Services::from_env("client-e2e");

    println!("building the shrink circuit...");
    let prover = Prover::new();

    let proving_started = Instant::now();
    let prepared = prover.prove(
        &find_log_action(),
        services.grounding_witness_without_inputs(),
    );
    let proving_time = proving_started.elapsed();
    println!("proved in {proving_time:.2?}");

    let timer = Instant::now();
    let mut submitted = [services.submit(prepared)];
    println!("submitted as relayer job {}", submitted[0].job_id);
    services.wait_for_landing(&mut submitted, timer);

    println!();
    println!("proving time: {proving_time:.2?}");
    println!("landing time: {:.2?}", submitted[0].landed_after.unwrap());
}
