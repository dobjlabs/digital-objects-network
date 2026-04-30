//! Top-level [`Driver`] — orchestrates the action lifecycle end to end.
//!
//! For each [`Driver::execute`] call:
//!
//! 1. Resolve input objects from the local store (by id or filename).
//! 2. Fetch the canonical state root + per-input grounding proofs from the
//!    synchronizer.
//! 3. Build the [`InputObject`]s with their two-level Merkle proofs.
//! 4. Hand off to the action's host-side preparer ([`crate::actions::*`])
//!    which constructs the new objects + intro witnesses (VDF, PoW, ...).
//! 5. Run the risc0 prover via [`crate::execute::prove_action`].
//! 6. Persist new objects as `.dobj` files (status = `Unknown` until relayer).
//! 7. Submit the receipt blob to the relayer.
//! 8. Poll until relayer confirms; flip output statuses to `Pending` then
//!    eventually `Live` once the synchronizer indexes the tx.
//! 9. Move consumed inputs to `.nullified`.
//!
//! The first three steps are wrapped in [`Driver::build_plan`] so tests can
//! exercise plan construction without real network calls.

use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use txlib_core::Hash;
use txlib_core::abi::{ActionId, InputObject};
use txlib_core::tx::StateRoot;

use crate::catalog::action_by_id;
use crate::clients::{
    HttpRelayerClient, HttpSynchronizerClient, JobStatus, RELAYER_POLL_INTERVAL_MS,
    RELAYER_POLL_TIMEOUT_SECS, RelayerClient, SYNCHRONIZER_POLL_INTERVAL_MS,
    SYNCHRONIZER_POLL_TIMEOUT_SECS, SynchronizerClient,
};
use crate::execute::{ExecutionPlan, Prover, Risc0Prover, prove_action};
use crate::object::{ObjectRecord, ObjectStatus, SourceTxData, sorted_commitments};
use crate::paths::{DriverPaths, default_paths};
use crate::settings::{DriverSettings, default_settings, read_settings, write_settings};
use crate::store;

/// What the caller hands to [`Driver::execute`].
#[derive(Debug, Clone)]
pub struct ExecuteActionInput {
    pub action_id: ActionId,
    pub input_selectors: Vec<ObjectSelector>,
    /// Action-specific staging built by the caller (the GUI or test). The
    /// driver doesn't peek inside — it just feeds it to the prover.
    pub staging: ActionStaging,
}

#[derive(Debug, Clone)]
pub enum ObjectSelector {
    FileName(String),
    Id(String),
}

/// Pre-prepared `new_objects` + `intro_witnesses` produced by the caller.
/// Building these is action-specific (PoW grinding, VDF chain) and lives
/// outside the driver — see `craft-actions::actions` for the validators
/// each one must satisfy.
#[derive(Debug, Clone)]
pub struct ActionStaging {
    pub new_objects: Vec<txlib_core::Object>,
    pub intro_witnesses: Vec<txlib_core::abi::IntroWitness>,
    /// Class names for each new object, in order. Used to name the output
    /// `.dobj` files. Must have the same length as `new_objects`.
    pub new_object_classes: Vec<String>,
}

/// Outcome of a successful end-to-end action execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteActionResult {
    pub action_id: ActionId,
    pub action_name: &'static str,
    pub tx_final: String,
    pub state_root_hash: String,
    pub relayer_job_id: String,
    pub tx_hash: Option<String>,
    pub block_number: Option<u64>,
    pub output_files: Vec<String>,
    pub nullified_files: Vec<String>,
}

/// Configuration for [`Driver::open`]. The defaults are wired up by
/// [`Driver::open_default`].
pub struct DriverDeps {
    pub synchronizer: Arc<dyn SynchronizerClient>,
    pub relayer: Arc<dyn RelayerClient>,
    pub prover: Arc<dyn Prover>,
}

pub struct Driver {
    pub paths: DriverPaths,
    pub settings: DriverSettings,
    pub deps: DriverDeps,
}

impl Driver {
    /// Open with the default `~/.dobj` layout, settings file (or defaults
    /// if missing), HTTP clients pointed at the URLs in settings, and the
    /// real risc0 prover.
    pub fn open_default() -> Result<Self> {
        let paths = default_paths()?;
        let settings = read_settings(&paths)?.unwrap_or_else(default_settings);
        let synchronizer: Arc<dyn SynchronizerClient> = Arc::new(HttpSynchronizerClient::new(
            settings.synchronizer_api_url.clone(),
        ));
        let relayer: Arc<dyn RelayerClient> = Arc::new(HttpRelayerClient::new(
            settings.relayer_api_url.clone(),
        ));
        let prover: Arc<dyn Prover> = Arc::new(Risc0Prover);
        Ok(Self {
            paths,
            settings,
            deps: DriverDeps {
                synchronizer,
                relayer,
                prover,
            },
        })
    }

    pub fn open(paths: DriverPaths, settings: DriverSettings, deps: DriverDeps) -> Self {
        Self {
            paths,
            settings,
            deps,
        }
    }

    pub fn save_settings(&self) -> Result<()> {
        write_settings(&self.paths, &self.settings)
    }

    // ------------------------------------------------------------------- read

    pub fn list_objects(&self) -> Result<Vec<ObjectRecord>> {
        store::list_live(&self.paths)?
            .iter()
            .map(|p| crate::object::parse_object_record_file(p))
            .collect()
    }

    pub fn read_object(&self, selector: &ObjectSelector) -> Result<ObjectRecord> {
        let resolved = self.resolve_one(selector)?;
        Ok(resolved.record)
    }

    // -------------------------------------------------------------- planning

    /// Build the [`ExecutionPlan`] for a given input — resolves object refs,
    /// fetches grounding proofs from the synchronizer, assembles the inputs
    /// list. Doesn't run the prover. Useful for tests.
    pub fn build_plan(&self, input: &ExecuteActionInput) -> Result<(ExecutionPlan, Vec<ResolvedInput>)> {
        let action = action_by_id(input.action_id)
            .ok_or_else(|| anyhow!("unknown action_id: {}", input.action_id))?;

        if action.inputs.len() != input.input_selectors.len() {
            return Err(anyhow!(
                "action {} expects {} inputs, got {}",
                action.name,
                action.inputs.len(),
                input.input_selectors.len()
            ));
        }
        if action.outputs.len() != input.staging.new_objects.len() {
            return Err(anyhow!(
                "action {} expects {} outputs, got {}",
                action.name,
                action.outputs.len(),
                input.staging.new_objects.len()
            ));
        }

        // Resolve every selector to a local record. Validate class against
        // the action's expected input class list, in order.
        let mut resolved: Vec<ResolvedInput> = Vec::with_capacity(input.input_selectors.len());
        for (i, sel) in input.input_selectors.iter().enumerate() {
            let r = self.resolve_one(sel)?;
            let expected_class = action.inputs[i];
            if r.record.class_name != expected_class {
                return Err(anyhow!(
                    "input #{i}: expected class {expected_class}, got {} (file {})",
                    r.record.class_name,
                    r.path.display()
                ));
            }
            if r.record.status == ObjectStatus::Nullified {
                return Err(anyhow!(
                    "input #{i} ({}) is already nullified",
                    r.path.display()
                ));
            }
            resolved.push(r);
        }

        // Fetch grounding witness from the synchronizer in one batched call.
        let source_tx_finals: Vec<Hash> =
            resolved.iter().map(|r| r.record.source_tx.tx_final()).collect();
        let witness = if source_tx_finals.is_empty() {
            // No inputs → no grounding needed; use the synchronizer's
            // current state head as the state_root the action grounds against.
            let head = self.deps.synchronizer.state_head()?;
            let gsr = head
                .current_gsr
                .ok_or_else(|| anyhow!("synchronizer has no canonical GSR yet"))?;
            // Build a placeholder StateRoot whose hash matches `gsr`. The
            // synchronizer trusts this on the way back: state_root.hash() is
            // checked against its recent_gsrs cache. For an inputless action
            // we don't actually use the transactions_root, but it has to be
            // consistent.
            //
            // Easier: don't use grounding-witness at all — just embed the
            // recent GSR. For inputs == 0 the guest's grounding loop is
            // empty so it doesn't matter what transactions_root / gsrs_root
            // we pass, only that `state_root.hash() == gsr`.
            //
            // We can recover the components from another grounding call
            // with no source txs; the synchronizer returns the canonical
            // roots regardless.
            let w = self.deps.synchronizer.grounding_witness(&[])?;
            // Sanity: the embedded roots must hash to the head's GSR.
            let sr = StateRoot::new(
                w.block_number,
                w.transactions_root,
                w.nullifiers_root,
                w.gsrs_root,
            );
            if sr.hash() != w.state_root_hash {
                return Err(anyhow!(
                    "synchronizer returned inconsistent grounding witness"
                ));
            }
            if sr.hash() != gsr {
                return Err(anyhow!(
                    "synchronizer head GSR {} doesn't match grounding witness {}",
                    gsr,
                    sr.hash()
                ));
            }
            crate::clients::GroundingWitness {
                witnesses: Vec::new(),
                ..w
            }
        } else {
            self.deps
                .synchronizer
                .grounding_witness(&source_tx_finals)?
        };

        // Pair each resolved input with its tx_inclusion_proof, build the
        // InputObject (which also rebuilds the live_inclusion_proof from
        // the `.dobj`'s sourceTxLive).
        let mut inputs: Vec<InputObject> = Vec::with_capacity(resolved.len());
        for (resolved_input, source_tx_final) in resolved.iter().zip(source_tx_finals.iter()) {
            let proof = witness
                .witnesses
                .iter()
                .find(|w| w.source_tx_final == *source_tx_final)
                .ok_or_else(|| {
                    anyhow!(
                        "synchronizer grounding witness missing entry for source tx {}",
                        source_tx_final
                    )
                })?;
            if !proof.present {
                return Err(anyhow!(
                    "source tx {} is not in the canonical transactions root",
                    source_tx_final
                ));
            }
            inputs.push(
                resolved_input
                    .record
                    .to_input_object(proof.tx_inclusion_proof.clone())?,
            );
        }

        let state_root = StateRoot::new(
            witness.block_number,
            witness.transactions_root,
            witness.nullifiers_root,
            witness.gsrs_root,
        );

        Ok((
            ExecutionPlan {
                action_id: input.action_id,
                state_root,
                inputs,
                new_objects: input.staging.new_objects.clone(),
                intro_witnesses: input.staging.intro_witnesses.clone(),
            },
            resolved,
        ))
    }

    // -------------------------------------------------------------- execution

    /// Run the full action lifecycle: build plan, prove, submit, persist,
    /// poll, nullify. Blocks for as long as it takes (could be minutes for
    /// real proving + on-chain confirmation).
    pub fn execute(&self, input: ExecuteActionInput) -> Result<ExecuteActionResult> {
        let action = action_by_id(input.action_id)
            .ok_or_else(|| anyhow!("unknown action_id"))?;

        let (plan, resolved) = self.build_plan(&input)?;

        // Snapshot the new-tx components BEFORE the prover consumes the
        // plan — the receipt's journal commits to `tx_final` but not the
        // individual roots, and the output `.dobj` records need them.
        let nullifiers = craft_actions::tx_build::nullifiers_for(&craft_input_view(&plan));
        let tx = craft_actions::tx_build::build_tx(&craft_input_view(&plan), &nullifiers);
        let new_obj_commitments = sorted_commitments(&input.staging.new_objects);

        let proved = prove_action(self.deps.prover.as_ref(), plan)?;

        let source_tx = SourceTxData {
            action_id: input.action_id,
            live_root: tx.live_root,
            nullifiers_root: tx.nullifiers_root,
            action_nonce: tx.action_nonce,
        };
        debug_assert_eq!(
            tx.tx_final(),
            proved.tx_final(),
            "host-built Tx must match the journal-committed tx_final"
        );

        let mut output_files = Vec::with_capacity(input.staging.new_objects.len());
        for (obj, class_name) in input
            .staging
            .new_objects
            .iter()
            .zip(input.staging.new_object_classes.iter())
        {
            let record = ObjectRecord::new(
                obj.clone(),
                class_name.clone(),
                source_tx.clone(),
                new_obj_commitments.clone(),
            );
            let path = store::write_live(&self.paths, &record)?;
            output_files.push(
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
        }

        // Submit the receipt to the relayer.
        let submission = self.deps.relayer.submit_payload(
            &proved.blob_payload,
            Some(action.name),
        )?;

        // Poll relayer until the blob is confirmed (or timed out).
        let final_status = self.poll_relayer(&submission.job_id)?;
        if final_status.status != JobStatus::Confirmed {
            return Err(anyhow!(
                "relayer job {} ended in non-confirmed state {:?}: {:?}",
                final_status.job_id,
                final_status.status,
                final_status.last_error
            ));
        }

        // Flip output statuses to Pending now that we have an eth tx hash.
        for obj in &input.staging.new_objects {
            store::update_by_commitment(&self.paths, obj.commitment(), |r| {
                r.status = ObjectStatus::Pending;
                r.tx_hash = final_status.tx_hash.clone();
            })?;
        }

        // Wait for synchronizer to index our tx_final.
        self.poll_synchronizer_for_tx(proved.tx_final())?;

        // Flip to Live.
        for obj in &input.staging.new_objects {
            store::update_by_commitment(&self.paths, obj.commitment(), |r| {
                r.status = ObjectStatus::Live;
            })?;
        }

        // Nullify consumed inputs.
        let mut nullified_files = Vec::with_capacity(resolved.len());
        for r in resolved {
            let nullified = store::nullify(&self.paths, &r.path)?;
            nullified_files.push(
                nullified
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
        }

        Ok(ExecuteActionResult {
            action_id: input.action_id,
            action_name: action.name,
            tx_final: format!("{}", proved.tx_final()),
            state_root_hash: format!("{}", proved.journal.state_root_hash),
            relayer_job_id: submission.job_id,
            tx_hash: final_status.tx_hash,
            block_number: final_status.block_number,
            output_files,
            nullified_files,
        })
    }

    // ----------------------------------------------------------------- helpers

    fn resolve_one(&self, selector: &ObjectSelector) -> Result<ResolvedInput> {
        match selector {
            ObjectSelector::Id(id) => {
                let (path, record) = store::find_by_id(&self.paths, id)?
                    .ok_or_else(|| anyhow!("no object with id {id}"))?;
                Ok(ResolvedInput { path, record })
            }
            ObjectSelector::FileName(name) => {
                let (path, record) = store::find_by_file_name(&self.paths, name)?
                    .ok_or_else(|| anyhow!("no object file named {name}"))?;
                Ok(ResolvedInput { path, record })
            }
        }
    }

    fn poll_relayer(&self, job_id: &str) -> Result<crate::clients::RelayJobStatus> {
        let deadline = Instant::now() + Duration::from_secs(RELAYER_POLL_TIMEOUT_SECS);
        loop {
            let status = self.deps.relayer.job_status(job_id)?;
            if status.status.is_terminal() {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "relayer poll timeout for job {job_id} (last status: {:?})",
                    status.status
                ));
            }
            sleep(Duration::from_millis(RELAYER_POLL_INTERVAL_MS));
        }
    }

    fn poll_synchronizer_for_tx(&self, tx_final: Hash) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(SYNCHRONIZER_POLL_TIMEOUT_SECS);
        loop {
            if self.deps.synchronizer.tx_present(tx_final)? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "synchronizer poll timeout: tx_final {tx_final} not observed"
                ));
            }
            sleep(Duration::from_millis(SYNCHRONIZER_POLL_INTERVAL_MS));
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedInput {
    pub path: std::path::PathBuf,
    pub record: ObjectRecord,
}

/// Borrow an `ExecutionPlan` as the `&GuestInput` view that
/// `craft_actions::tx_build::build_tx` expects.
fn craft_input_view(plan: &ExecutionPlan) -> txlib_core::abi::GuestInput {
    txlib_core::abi::GuestInput {
        action_id: plan.action_id,
        state_root: plan.state_root.clone(),
        inputs: plan.inputs.clone(),
        new_objects: plan.new_objects.clone(),
        intro_witnesses: plan.intro_witnesses.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::action_by_name;

    #[test]
    fn execute_action_input_struct_compiles() {
        let action = action_by_name("FindLog").unwrap();
        let _ = ExecuteActionInput {
            action_id: action.id,
            input_selectors: vec![],
            staging: ActionStaging {
                new_objects: vec![],
                intro_witnesses: vec![],
                new_object_classes: vec![],
            },
        };
    }
}
