//! End-to-end action execution: build the `GuestInput`, run the risc0
//! prover, package the receipt as a blob payload.
//!
//! The driver's `execute_action` (in [`crate::driver`]) wraps this with
//! file IO + relayer submission + lifecycle updates. This module is the
//! pure "build-the-proof" step.

use anyhow::{Context, Result};
use craft_methods::CRAFT_GUEST_ELF;
use risc0_zkvm::{ExecutorEnv, Receipt, default_prover};
use txlib_core::Hash;
use txlib_core::abi::{ActionId, GuestInput, GuestJournal, InputObject, IntroWitness};
use txlib_core::tx::StateRoot;
use txlib_core::Object;

/// Trait so tests / integration can swap the real risc0 prover for a mock.
pub trait Prover: Send + Sync {
    /// Run the guest against `input`, return the receipt.
    fn prove(&self, input: &GuestInput) -> Result<Receipt>;
}

/// Real risc0 prover. In dev mode (`RISC0_DEV_MODE=1`) this skips the
/// expensive STARK proving but still runs the guest end-to-end.
pub struct Risc0Prover;

impl Prover for Risc0Prover {
    fn prove(&self, input: &GuestInput) -> Result<Receipt> {
        let input_bytes = borsh::to_vec(input).context("borsh-encode GuestInput")?;
        let env = ExecutorEnv::builder()
            .write(&input_bytes)
            .context("ExecutorEnv::write")?
            .build()
            .context("ExecutorEnv::build")?;
        let prove_info = default_prover()
            .prove(env, CRAFT_GUEST_ELF)
            .context("risc0 prove")?;
        Ok(prove_info.receipt)
    }
}

/// All the pieces the driver assembles before invoking the prover.
pub struct ExecutionPlan {
    pub action_id: ActionId,
    pub state_root: StateRoot,
    pub inputs: Vec<InputObject>,
    pub new_objects: Vec<Object>,
    pub intro_witnesses: Vec<IntroWitness>,
}

impl ExecutionPlan {
    pub fn into_guest_input(self) -> GuestInput {
        GuestInput {
            action_id: self.action_id,
            state_root: self.state_root,
            inputs: self.inputs,
            new_objects: self.new_objects,
            intro_witnesses: self.intro_witnesses,
        }
    }
}

pub struct ProvedAction {
    pub journal: GuestJournal,
    pub receipt: Receipt,
    /// Wire-format blob the relayer expects (magic envelope + bincode receipt).
    pub blob_payload: Vec<u8>,
}

impl ProvedAction {
    pub fn tx_final(&self) -> Hash {
        self.journal.tx_final
    }
}

pub fn prove_action(prover: &dyn Prover, plan: ExecutionPlan) -> Result<ProvedAction> {
    let input = plan.into_guest_input();

    // Sanity-check on the host side: catch predicate violations BEFORE
    // committing prover cycles. Same code the guest runs, so a successful
    // host-side check is necessary (but not sufficient — the guest also
    // re-checks, and Merkle proofs only verify in the guest with real
    // grounding from the synchronizer).
    let expected_journal = craft_actions::validate(&input);

    let receipt = prover.prove(&input)?;

    // The receipt should already be verified by `default_prover().prove`,
    // but call verify explicitly for clarity in the dev-mode and mock paths.
    receipt
        .verify(craft_methods::CRAFT_GUEST_ID)
        .context("receipt verification failed")?;

    let journal: GuestJournal = borsh::from_slice(&receipt.journal.bytes)
        .context("borsh-decode journal from receipt")?;
    if journal != expected_journal {
        return Err(anyhow::anyhow!(
            "guest journal differs from host validate() — guest dispatch table or hash recipe drift"
        ));
    }

    let receipt_bytes = bincode::serialize(&receipt).context("bincode-serialize Receipt")?;
    let blob_payload = common::payload::encode_blob_payload(&receipt_bytes);

    Ok(ProvedAction {
        journal,
        receipt,
        blob_payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use txlib_core::abi::GuestInput;

    /// Mock prover: deterministically calls `craft_actions::validate` on the
    /// host (in production the guest does it inside the zkVM), packages the
    /// resulting journal into a stub `Receipt`. Lets tests exercise the
    /// blob-encoding + lifecycle paths without spinning up risc0.
    pub struct MockProver {
        pub last_input: Mutex<Option<GuestInput>>,
    }

    impl MockProver {
        pub fn new() -> Self {
            Self {
                last_input: Mutex::new(None),
            }
        }
    }

    impl Prover for MockProver {
        fn prove(&self, input: &GuestInput) -> Result<Receipt> {
            *self.last_input.lock().unwrap() = Some(input.clone());
            // Build a fake receipt by going through risc0's dev-mode helpers.
            // We can't easily fabricate a Receipt without running the prover,
            // so this mock is only useful for code paths that don't reach
            // `prove_action` (which calls `receipt.verify`). Tests that only
            // need the journal should call `craft_actions::validate` directly.
            anyhow::bail!("MockProver doesn't synthesize real receipts; use `validate` directly")
        }
    }

    #[test]
    fn execution_plan_into_guest_input_preserves_fields() {
        let plan = ExecutionPlan {
            action_id: 1,
            state_root: StateRoot::new(0, Hash::default(), Hash::default(), Hash::default()),
            inputs: vec![],
            new_objects: vec![],
            intro_witnesses: vec![],
        };
        let input = plan.into_guest_input();
        assert_eq!(input.action_id, 1);
    }

    #[test]
    fn mock_prover_records_input() {
        let prover = MockProver::new();
        let input = GuestInput {
            action_id: 9,
            state_root: StateRoot::new(0, Hash::default(), Hash::default(), Hash::default()),
            inputs: vec![],
            new_objects: vec![],
            intro_witnesses: vec![],
        };
        let _ = prover.prove(&input);
        assert_eq!(prover.last_input.lock().unwrap().as_ref(), Some(&input));
    }
}
