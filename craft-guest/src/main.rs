//! Unified risc0 guest binary for the craft-basics action set.
//!
//! Reads a borsh-encoded [`GuestInput`] from the host, runs grounding +
//! action validation + tx building via [`craft_actions::validate`], and
//! commits a borsh-encoded [`GuestJournal`] to the receipt journal.
//!
//! Any panic — invalid grounding, predicate violation, type error in the
//! input — aborts proving and produces no receipt. Synchronizer + driver
//! interpret a missing receipt as "the action wasn't possible".

#![no_main]

use risc0_zkvm::guest::env;
use txlib_core::abi::{GuestInput, GuestJournal};

risc0_zkvm::guest::entry!(main);

fn main() {
    // 1. Read the input. Borsh — same format as the host's
    //    `bincode::serialize(&GuestInput)` would *not* be (we use borsh
    //    intentionally so the encoding is canonical and matches what the
    //    rest of the crate uses).
    let input_bytes: Vec<u8> = env::read();
    let input: GuestInput = borsh::from_slice(&input_bytes)
        .expect("failed to borsh-decode GuestInput");

    // 2. Run validation + tx assembly. Panics on any predicate violation.
    let journal: GuestJournal = craft_actions::validate(&input);

    // 3. Commit the journal as raw borsh bytes. The synchronizer's
    //    `Risc0Verifier` decodes these via `borsh::from_slice(&receipt.journal.bytes)`.
    let journal_bytes = borsh::to_vec(&journal).expect("failed to borsh-encode GuestJournal");
    env::commit_slice(&journal_bytes);
}
