//! Transaction proving, recording, submission, deletion, and settlement.
//! `execute` composes `prove`, `record`, and `submit`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use payload::encode_hash_hex;
use pod2::middleware::{Hash, containers::Dictionary};
use txlib::{MAX_STATE_ROOT_AGE_BLOCKS, object_nullifier_hash};
use wire_types::relayer::JobStatus;
use wire_types::transactions::{TransactionRecord, TransactionState};
use wire_types::{ExecutionPhase, ObjectStatus, ObjectSummary, QualifiedName};

use crate::driver::Driver;
use crate::error::DriverError;
use crate::execute::{decode_action_args, resolve_inputs, validate_execute_request};
use crate::holds::Holds;
use crate::object_record::{FileStatus, ObjectRecord};
use crate::object_store::{
    ObjectFileEntry, load_object_files, matches_query, object_file_name, write_object_file,
};
use crate::transactions::{
    decode_hashes, load_records, locked_inputs, read_record, record_tx_final, remove_transaction,
    write_payload, write_record,
};
use crate::types::{
    ExecuteActionInput, ExecuteActionResult, ExecutionReporter, ExecutionStepContext,
    NoopExecutionReporter, ObjectQuery,
};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// One output of a proved transaction, not yet written to disk.
#[derive(Debug, Clone)]
pub struct ProvedOutput {
    pub class: QualifiedName,
    pub obj: Dictionary,
}

/// Proved payload and outputs, with input holds retained until recording
/// completes or this value is dropped.
#[derive(Debug)]
pub struct ProvedTransaction {
    pub tx_final: Hash,
    pub action: QualifiedName,
    pub grounding_root: Hash,
    pub grounding_block: i64,
    pub input_files: Vec<String>,
    pub outputs: Vec<ProvedOutput>,
    pub nullifiers: Vec<Hash>,
    pub created: Vec<Hash>,
    pub payload_bytes: Vec<u8>,
    /// Kept for its release on drop.
    _holds: Holds,
}

impl ProvedTransaction {
    pub fn output_files(&self) -> Result<Vec<String>> {
        self.outputs
            .iter()
            .map(|output| object_file_name(&output.class, &output.obj))
            .collect()
    }
}

/// What one settlement pass changed.
#[derive(Debug, Clone, Default)]
pub struct SettleOutcome {
    pub records: Vec<TransactionRecord>,
    pub objects: Vec<ObjectSummary>,
}

impl Driver {
    /// Prove and shrink an action while holding its inputs under `owner`.
    /// Refuse inputs held by another proof or spent by an open transaction.
    pub fn prove<R: ExecutionReporter>(
        &self,
        input: ExecuteActionInput,
        owner: &str,
        reporter: &R,
    ) -> Result<ProvedTransaction> {
        let settings = self.load_settings()?;
        // Use one catalog version for both action lookup and execution.
        let catalog = self.catalog();
        let action = catalog
            .get_action(&input.action)
            .ok_or_else(|| DriverError::UnknownAction(input.action.to_string()))?;
        validate_execute_request(&input, &action)?;
        let args = decode_action_args(&input.args)?;

        let no_ctx = ExecutionStepContext::default();
        reporter.on_step(ExecutionPhase::GenerateProof, "Verifying inputs", &no_ctx);
        let (resolved_inputs, holds) = {
            let _store = self.store_lock.lock().expect("store lock poisoned");
            let entries = load_object_files(&self.paths)?;
            let resolved_inputs = resolve_inputs(&entries, &input, &action)?;
            let input_files = resolved_inputs
                .iter()
                .map(|resolved| resolved.file_name.clone())
                .collect::<Vec<_>>();
            let locked = locked_inputs(&load_records(&self.paths)?);
            let holds = self.holds.take(&input_files, owner, &locked)?;
            (resolved_inputs, holds)
        };

        let input_commitments = resolved_inputs
            .iter()
            .map(|entry| entry.record.obj.commitment())
            .collect::<Vec<_>>();
        let grounding_witness = self
            .deps
            .synchronizer
            .fetch_grounding_witness(&settings.synchronizer_api_url, &input_commitments)?;
        let grounding_root = grounding_witness.state_header.hash();
        let grounding_block = grounding_witness.state_header.block_number;

        reporter.on_step(ExecutionPhase::GenerateProof, "Generating proof", &no_ctx);
        let execution_inputs = resolved_inputs
            .iter()
            .map(|input| input.record.spendable())
            .collect::<Vec<_>>();
        let spendable_outputs = catalog.execute_action(
            input.action.clone(),
            grounding_witness,
            execution_inputs,
            &args,
        )?;
        if spendable_outputs.objs.len() != action.total_outputs.len() {
            return Err(anyhow!(
                "action {} output mismatch: descriptor expects {}, engine returned {}",
                action.action,
                action.total_outputs.len(),
                spendable_outputs.objs.len()
            ));
        }
        reporter.on_done(ExecutionPhase::GenerateProof, None);

        let shrink_ctx = ExecutionStepContext {
            old_root: Some(grounding_root),
            output_files: Vec::new(),
            output_status: None,
        };
        reporter.on_step(ExecutionPhase::Commit, "Shrinking proof", &shrink_ctx);
        let payload_bytes = self
            .deps
            .payload_builder
            .build_payload(&grounding_root, &spendable_outputs)?;

        let outputs = action
            .total_outputs
            .iter()
            .zip(spendable_outputs.objs.iter())
            .map(|(output, spendable)| ProvedOutput {
                class: output.class.clone(),
                obj: spendable.obj.clone(),
            })
            .collect();
        Ok(ProvedTransaction {
            tx_final: spendable_outputs.tx.dict().commitment(),
            action: input.action,
            grounding_root,
            grounding_block,
            input_files: holds.files().to_vec(),
            outputs,
            nullifiers: spendable_outputs.tx.nullifier_hashes()?,
            created: spendable_outputs.tx.created_commitments()?,
            payload_bytes,
            _holds: holds,
        })
    }

    /// Persist unknown outputs and payload, then commit the record.
    /// Recheck input availability and output collisions under the store lock.
    /// Release input holds on success or failure.
    pub fn record(&self, proved: ProvedTransaction) -> Result<TransactionRecord> {
        let _store = self.store_lock.lock().expect("store lock poisoned");
        let output_files = proved.output_files()?;
        let entries = load_object_files(&self.paths)?;
        let locked = locked_inputs(&load_records(&self.paths)?);
        for input in &proved.input_files {
            if locked.contains(input) {
                return Err(DriverError::Conflict(format!(
                    "input {input} is spent by a transaction awaiting the chain"
                ))
                .into());
            }
            match entries.iter().find(|entry| &entry.file_name == input) {
                Some(entry) if entry.record.status == FileStatus::Live => {}
                Some(entry) => {
                    return Err(DriverError::Conflict(format!(
                        "input {input} is no longer live (status: {:?})",
                        entry.record.status
                    ))
                    .into());
                }
                None => return Err(DriverError::ObjectFileNotFound(input.clone()).into()),
            }
        }
        for output in &output_files {
            if entries.iter().any(|entry| &entry.file_name == output) {
                return Err(
                    DriverError::Conflict(format!("output file already exists: {output}")).into(),
                );
            }
        }

        for (output, file_name) in proved.outputs.iter().zip(&output_files) {
            let record = ObjectRecord {
                content_hash: output.obj.commitment(),
                class: output.class.clone(),
                status: FileStatus::Unknown,
                obj: output.obj.clone(),
                tx_final: proved.tx_final,
            };
            write_object_file(&self.paths, &record, file_name)?;
        }
        write_payload(&self.paths, &proved.tx_final, &proved.payload_bytes)?;
        let record = TransactionRecord {
            tx_final: encode_hash_hex(&proved.tx_final),
            action: proved.action.clone(),
            input_files: proved.input_files.clone(),
            output_files,
            grounding_root: encode_hash_hex(&proved.grounding_root),
            grounding_block: proved.grounding_block,
            nullifiers: proved.nullifiers.iter().map(encode_hash_hex).collect(),
            created: proved.created.iter().map(encode_hash_hex).collect(),
            state: TransactionState::Recorded,
            relayer_job_id: None,
            tx_hash: None,
            relayer_failure: None,
            reason: None,
            recorded_at_ms: now_ms(),
            submit_attempted_at_ms: None,
            submitted_at_ms: None,
            settled_at_ms: None,
        };
        write_record(&self.paths, &record)?;
        Ok(record)
    }

    /// Delete a `recorded` transaction and its unknown outputs. Refuse once
    /// submission was attempted: the relayer may have accepted the payload.
    pub fn delete_transaction(&self, tx_final: &Hash) -> Result<TransactionRecord> {
        let _store = self.store_lock.lock().expect("store lock poisoned");
        let record = read_record(&self.paths, tx_final)?
            .ok_or_else(|| DriverError::TransactionNotFound(encode_hash_hex(tx_final)))?;
        if record.state != TransactionState::Recorded {
            return Err(DriverError::Conflict(format!(
                "transaction {} is {}, only recorded transactions can be deleted",
                record.tx_final,
                state_name(record.state)
            ))
            .into());
        }
        if record.submit_attempted_at_ms.is_some() {
            return Err(DriverError::Conflict(format!(
                "transaction {} was offered to the relayer and may still land; it ends when the chain decides",
                record.tx_final
            ))
            .into());
        }
        self.remove_unknown_outputs(&record, tx_final)?;
        remove_transaction(&self.paths, tx_final)?;
        Ok(record)
    }

    /// Submit a recorded payload and persist the relayer job ID.
    /// Refuse expired groundings; return submitted records unchanged.
    pub fn submit(&self, tx_final: &Hash) -> Result<TransactionRecord> {
        let settings = self.load_settings()?;
        let record = read_record(&self.paths, tx_final)?
            .ok_or_else(|| DriverError::TransactionNotFound(encode_hash_hex(tx_final)))?;
        match record.state {
            TransactionState::Recorded => {}
            TransactionState::Submitted => return Ok(record),
            state => {
                return Err(DriverError::Conflict(format!(
                    "transaction {} is {}",
                    record.tx_final,
                    state_name(state)
                ))
                .into());
            }
        }
        let head = self
            .deps
            .synchronizer
            .fetch_head(&settings.synchronizer_api_url)?;
        if aged_out(head.block_number, record.grounding_block) {
            return Err(DriverError::Conflict(format!(
                "transaction {} is grounded at block {}, more than {MAX_STATE_ROOT_AGE_BLOCKS} blocks behind the synchronizer head {}",
                record.tx_final, record.grounding_block, head.block_number
            ))
            .into());
        }

        let payload_bytes = crate::transactions::read_payload(&self.paths, tx_final)?;
        let attempted = self.update_record(tx_final, |record| {
            if record.state != TransactionState::Recorded {
                return Ok(false);
            }
            record.submit_attempted_at_ms.get_or_insert_with(now_ms);
            Ok(true)
        })?;
        if !attempted.1 {
            return Ok(attempted.0);
        }

        let response = self.deps.relayer.submit_proof(
            &settings.relayer_api_url,
            &payload_bytes,
            Some(format!("driver:{}", record.action)),
        )?;
        let (record, _) = self.update_record(tx_final, |record| {
            if record.state != TransactionState::Recorded {
                return Ok(false);
            }
            record.state = TransactionState::Submitted;
            record.relayer_job_id = Some(response.job_id.clone());
            record.submitted_at_ms = Some(now_ms());
            if response.status == JobStatus::Failed {
                record.relayer_failure = Some("relayer job failed".to_string());
            }
            Ok(true)
        })?;
        Ok(record)
    }

    /// Reconcile open records against membership and relayer status, then
    /// persist changes under the store lock.
    pub fn settle(&self) -> Result<SettleOutcome> {
        self.settle_with(false).map(|(outcome, _)| outcome)
    }

    /// Reconcile open transactions and all object files; return the object listing.
    pub fn sync_objects(&self, query: Option<&ObjectQuery>) -> Result<Vec<ObjectSummary>> {
        let (_, summaries) = self.settle_with(true)?;
        Ok(summaries
            .into_iter()
            .filter(|(entry, summary)| {
                query.is_none_or(|query| matches_query(entry, summary.status, query))
            })
            .map(|(_, summary)| summary)
            .collect())
    }

    fn settle_with(
        &self,
        all_objects: bool,
    ) -> Result<(SettleOutcome, Vec<(ObjectFileEntry, ObjectSummary)>)> {
        let settings = self.load_settings()?;
        let (records, entries) = {
            let _store = self.store_lock.lock().expect("store lock poisoned");
            (load_records(&self.paths)?, load_object_files(&self.paths)?)
        };
        let open_records = records
            .iter()
            .filter(|record| record.is_open())
            .collect::<Vec<_>>();

        let mut commitments = HashSet::new();
        let mut nullifiers = HashSet::new();
        for record in &open_records {
            commitments.extend(decode_hashes(&record.created)?);
            nullifiers.extend(decode_hashes(&record.nullifiers)?);
        }
        if all_objects {
            for entry in &entries {
                commitments.insert(entry.record.obj.commitment());
                if let Ok(nullifier) = object_nullifier_hash(&entry.record.obj) {
                    nullifiers.insert(nullifier);
                }
            }
        }
        if commitments.is_empty() && nullifiers.is_empty() {
            let summaries = self.summaries(&entries, &records);
            return Ok((SettleOutcome::default(), summaries));
        }

        let membership = self.deps.synchronizer.fetch_membership_with_nullifiers(
            &settings.synchronizer_api_url,
            &commitments.into_iter().collect::<Vec<_>>(),
            &nullifiers.into_iter().collect::<Vec<_>>(),
        )?;
        let mut relayer_jobs = HashMap::new();
        for record in &open_records {
            let Some(job_id) = record
                .relayer_job_id
                .as_deref()
                .filter(|_| record.state == TransactionState::Submitted)
            else {
                continue;
            };
            match self
                .deps
                .relayer
                .fetch_job_status(&settings.relayer_api_url, job_id)
            {
                Ok(status) => {
                    relayer_jobs.insert(record.tx_final.clone(), status);
                }
                Err(err) => log::warn!("relayer status for job {job_id} unavailable: {err:#}"),
            }
        }

        let _store = self.store_lock.lock().expect("store lock poisoned");
        let mut outcome = SettleOutcome::default();
        let settled_at_ms = now_ms();
        let mut records = load_records(&self.paths)?;
        for record in records.iter_mut().filter(|record| record.is_open()) {
            let before = record.clone();
            if let Some(job) = relayer_jobs.get(&record.tx_final) {
                if job.tx_hash.is_some() {
                    record.tx_hash = job.tx_hash.clone();
                }
                if job.status == JobStatus::Failed {
                    record.relayer_failure = Some(
                        job.last_error
                            .clone()
                            .unwrap_or_else(|| "relayer job failed".to_string()),
                    );
                }
            }
            let created = decode_hashes(&record.created)?;
            let spent = decode_hashes(&record.nullifiers)?;
            let landed = created
                .iter()
                .all(|commitment| membership.created_objects.contains(commitment))
                && spent
                    .iter()
                    .all(|nullifier| membership.on_chain_nullifiers.contains(nullifier));
            if landed {
                record.state = TransactionState::Landed;
                record.settled_at_ms = Some(settled_at_ms);
            } else if aged_out(membership.block_number, record.grounding_block) {
                record.state = TransactionState::Expired;
                record.reason = Some(format!(
                    "grounding block {} is more than {MAX_STATE_ROOT_AGE_BLOCKS} blocks behind the synchronizer head {}",
                    record.grounding_block, membership.block_number
                ));
                record.settled_at_ms = Some(settled_at_ms);
                self.remove_unknown_outputs(record, &record_tx_final(record)?)?;
            }
            if *record != before {
                write_record(&self.paths, record)?;
                outcome.records.push(record.clone());
            }
        }

        let mut entries = load_object_files(&self.paths)?;
        let mut changed_files = HashSet::new();
        for entry in entries.iter_mut() {
            let commitment = entry.record.obj.commitment();
            let nullifier = object_nullifier_hash(&entry.record.obj).ok();
            let status = match entry.record.status {
                FileStatus::Nullified => continue,
                _ if nullifier.is_some_and(|n| membership.on_chain_nullifiers.contains(&n)) => {
                    FileStatus::Nullified
                }
                FileStatus::Unknown if membership.created_objects.contains(&commitment) => {
                    FileStatus::Live
                }
                _ => continue,
            };
            entry.record.status = status;
            write_object_file(&self.paths, &entry.record, &entry.file_name)?;
            changed_files.insert(entry.file_name.clone());
        }
        for record in &outcome.records {
            if record.state.is_terminal() {
                changed_files.extend(record.output_files.iter().cloned());
                changed_files.extend(record.input_files.iter().cloned());
            }
        }
        let summaries = self.summaries(&entries, &records);
        outcome.objects = summaries
            .iter()
            .filter(|(entry, _)| changed_files.contains(&entry.file_name))
            .map(|(_, summary)| summary.clone())
            .collect();
        Ok((outcome, summaries))
    }

    /// Basename to the label of the proof holding it as an input.
    pub fn held_inputs(&self) -> HashMap<String, String> {
        self.holds.labels()
    }

    pub fn list_transactions(&self) -> Result<Vec<TransactionRecord>> {
        load_records(&self.paths)
    }

    pub fn get_transaction(&self, tx_final: &Hash) -> Result<TransactionRecord> {
        read_record(&self.paths, tx_final)?
            .ok_or_else(|| DriverError::TransactionNotFound(encode_hash_hex(tx_final)).into())
    }

    pub fn execute(&self, input: ExecuteActionInput) -> Result<ExecuteActionResult> {
        self.execute_with_reporter(input, &NoopExecutionReporter)
    }

    /// `prove`, `record` and `submit` in one call. Landing is reported by
    /// settlement.
    pub fn execute_with_reporter<R: ExecutionReporter>(
        &self,
        input: ExecuteActionInput,
        reporter: &R,
    ) -> Result<ExecuteActionResult> {
        let owner = format!("execute:{}", input.action);
        let proved = self.prove(input, &owner, reporter)?;
        let old_root = proved.grounding_root;
        let mut ctx = ExecutionStepContext {
            old_root: Some(old_root),
            output_files: Vec::new(),
            output_status: None,
        };
        reporter.on_step(ExecutionPhase::Commit, "Creating files", &ctx);
        let record = self.record(proved)?;
        ctx.output_files = record.output_files.clone();
        ctx.output_status = Some(ObjectStatus::Unknown);
        reporter.on_step(
            ExecutionPhase::Commit,
            "Output object files created with status unknown",
            &ctx,
        );
        reporter.on_step(ExecutionPhase::Commit, "Submitting proof to relayer", &ctx);
        let tx_final = record_tx_final(&record)?;
        let submitted = self.submit(&tx_final)?;
        let relayer_job_id = submitted
            .relayer_job_id
            .clone()
            .ok_or_else(|| anyhow!("submitted record {} has no job id", submitted.tx_final))?;
        ctx.output_status = Some(ObjectStatus::Pending);
        reporter.on_step(
            ExecutionPhase::Commit,
            &format!("Relayer accepted job {relayer_job_id}; landing is reported by settlement"),
            &ctx,
        );
        let result = ExecuteActionResult {
            tx_final,
            old_root,
            output_files: record.output_files,
            input_files: record.input_files,
            relayer_job_id,
        };
        reporter.on_done(ExecutionPhase::Commit, Some(&result));
        Ok(result)
    }

    /// Apply `change` to a fresh record under the store lock; persist only
    /// when the callback returns true.
    fn update_record(
        &self,
        tx_final: &Hash,
        change: impl FnOnce(&mut TransactionRecord) -> Result<bool>,
    ) -> Result<(TransactionRecord, bool)> {
        let _store = self.store_lock.lock().expect("store lock poisoned");
        let mut record = read_record(&self.paths, tx_final)?
            .ok_or_else(|| DriverError::TransactionNotFound(encode_hash_hex(tx_final)))?;
        let changed = change(&mut record)?;
        if changed {
            write_record(&self.paths, &record)?;
        }
        Ok((record, changed))
    }

    /// Removes the record's output files that are still `unknown` and
    /// carry its txFinal. Caller holds the store lock.
    fn remove_unknown_outputs(&self, record: &TransactionRecord, tx_final: &Hash) -> Result<()> {
        for file_name in &record.output_files {
            let path = self.paths.objects_dir.join(file_name);
            let Ok(stored) = crate::object_record::parse_object_record_file(&path) else {
                continue;
            };
            if stored.status == FileStatus::Unknown && stored.tx_final == *tx_final {
                fs::remove_file(&path)
                    .with_context(|| format!("failed to remove output {file_name}"))?;
            }
        }
        Ok(())
    }
}

pub(crate) fn aged_out(head_block: i64, grounding_block: i64) -> bool {
    head_block - grounding_block > MAX_STATE_ROOT_AGE_BLOCKS
}

fn state_name(state: TransactionState) -> &'static str {
    match state {
        TransactionState::Recorded => "recorded",
        TransactionState::Submitted => "submitted",
        TransactionState::Landed => "landed",
        TransactionState::Expired => "expired",
    }
}
