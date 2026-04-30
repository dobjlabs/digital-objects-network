//! Smoke test: prove + verify a `FindLog` action end-to-end.
//!
//! Usage:
//! ```bash
//! # Dev mode (no real proof, ~milliseconds — for iterating):
//! RISC0_DEV_MODE=1 cargo run -p craft-methods --example prove_find_log
//!
//! # Real proof (~minutes on Apple Silicon):
//! cargo run -p craft-methods --release --example prove_find_log
//! ```

use anyhow::{Context, Result};
use craft_actions::{ACTION_FIND_LOG, actions::USE_WOOD_PICK_VDF_ITERS};
use craft_methods::{CRAFT_GUEST_ELF, CRAFT_GUEST_ID, image_id_hex};
use risc0_zkvm::{ExecutorEnv, default_prover};
use txlib_core::Hash;
use txlib_core::abi::{GuestInput, GuestJournal, IntroWitness};
use txlib_core::hash::sha256;
use txlib_core::object;
use txlib_core::tx::StateRoot;

const FIND_LOG_VDF_ITERS: u32 = 3;

fn run_vdf(iters: u32, input: Hash) -> Hash {
    let mut current = input;
    for _ in 0..iters {
        current = sha256(current.as_bytes());
    }
    current
}

fn build_find_log_input() -> GuestInput {
    // FindLog takes 0 inputs and produces 1 Log object whose `work` field
    // is the SHA-256 chain of length 3 over the log's pre-work commitment.
    let key = sha256(b"smoke-test-key");
    let mut log = object! {
        "blueprint" => "Log",
        "key" => key,
    };
    let vdf_input = log.commitment();
    let work = run_vdf(FIND_LOG_VDF_ITERS, vdf_input);
    log.insert("work", work);

    GuestInput {
        action_id: ACTION_FIND_LOG,
        // Empty state root: FindLog has no inputs to ground.
        state_root: StateRoot::new(0, Hash::default(), Hash::default(), Hash::default()),
        inputs: Vec::new(),
        new_objects: vec![log],
        intro_witnesses: vec![IntroWitness::Vdf {
            iters: FIND_LOG_VDF_ITERS,
            input: vdf_input,
            output: work,
        }],
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::filter::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    eprintln!("image_id: 0x{}", image_id_hex());

    // 1. Build the input the host would normally assemble from .dobj files
    //    + the synchronizer's grounding witness.
    let input = build_find_log_input();
    let input_bytes = borsh::to_vec(&input).context("borsh-encode GuestInput")?;
    eprintln!("input: {} bytes", input_bytes.len());

    // 2. Set up the executor environment. The guest reads via env::read(),
    //    which deserialies Vec<u8> from the input — exactly the
    //    borsh-encoded GuestInput we wrote.
    let env = ExecutorEnv::builder()
        .write(&input_bytes)
        .context("ExecutorEnv::write")?
        .build()
        .context("ExecutorEnv::build")?;

    // 3. Prove. In dev mode this skips actual proving (~ms); otherwise
    //    runs the real STARK prover (~minutes on consumer hardware).
    let start = std::time::Instant::now();
    let prove_info = default_prover()
        .prove(env, CRAFT_GUEST_ELF)
        .context("prove")?;
    eprintln!("prove: {:?}", start.elapsed());

    let receipt = prove_info.receipt;

    // 4. Verify the receipt against the pinned image_id.
    receipt.verify(CRAFT_GUEST_ID).context("verify")?;
    eprintln!("verify: ok");

    // 5. Decode the journal and check the publicly committed values.
    let journal: GuestJournal = borsh::from_slice(&receipt.journal.bytes)
        .context("borsh-decode GuestJournal from receipt journal")?;

    eprintln!("journal:");
    eprintln!("  state_root_hash: {}", journal.state_root_hash);
    eprintln!("  tx_final:        {}", journal.tx_final);
    eprintln!("  nullifiers:      {} entries", journal.nullifiers.len());

    // Cross-check against what running the validator on the host would say.
    let expected = craft_actions::validate(&input);
    assert_eq!(journal, expected, "guest journal must match host validate()");
    eprintln!("journal == host validate(input): ok");

    // Confirm USE_WOOD_PICK_VDF_ITERS is available — touches the actions
    // module so Cargo links the const we re-exported.
    let _ = USE_WOOD_PICK_VDF_ITERS;

    Ok(())
}
