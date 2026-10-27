//! Transaction proving, recording, submission, deletion, and settlement.
//! `execute` composes `prove`, `record`, and `submit`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use payload::encode_hash_hex;
use pod2::middleware::{Hash, containers::Dictionary};
use txlib::{grounding_aged_out, object_nullifier_hash};
use wire_types::relayer::JobStatus;
use wire_types::transactions::{TransactionRecord, TransactionState};
use wire_types::{ExecutionPhase, ExecutionStage, ObjectSummary, QualifiedName};

use crate::driver::Driver;
use crate::error::DriverError;
use crate::execute::{decode_action_args, resolve_inputs, validate_execute_request};
use crate::holds::Holds;
use crate::object_record::{FileStatus, ObjectRecord};
use crate::object_store::{load_object_files, matches_query, object_file_name, write_object_file};
use crate::transactions::{
    decode_hashes, load_records, locked_inputs, read_record, record_tx_final, remove_transaction,
    write_payload, write_record,
};
use crate::types::{ExecuteActionInput, ExecutionReporter, ExecutionStepContext, ObjectQuery};

/// Unix time in milliseconds; returns zero for times before the epoch.
pub fn now_ms() -> u64 {
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
    pub owner: String,
    pub grounding_root: Hash,
    pub grounding_block: i64,
    pub outputs: Vec<ProvedOutput>,
    pub nullifiers: Vec<Hash>,
    pub created: Vec<Hash>,
    pub payload_bytes: Vec<u8>,
    holds: Holds,
}

impl ProvedTransaction {
    pub fn input_files(&self) -> &[String] {
        self.holds.files()
    }

    pub fn output_files(&self) -> Result<Vec<String>> {
        self.outputs
            .iter()
            .map(|output| object_file_name(&output.class, &output.obj))
            .collect()
    }
}

/// Changed records and objects, removed files, and the remaining open count.
#[derive(Debug, Clone, Default)]
pub struct SettleOutcome {
    pub records: Vec<TransactionRecord>,
    pub objects: Vec<ObjectSummary>,
    /// Output files of expired transactions, removed by this pass.
    pub removed_files: Vec<String>,
    pub open_records: usize,
    pub unattempted_records: usize,
    pub pruned_tx_finals: Vec<String>,
    pub next_cleanup_at_ms: Option<u64>,
}

pub const TRANSACTION_RETENTION_MS: u64 = 600_000;

#[derive(Debug, Clone, Default)]
pub struct PrunedTransactions {
    pub tx_finals: Vec<String>,
    pub objects: Vec<ObjectSummary>,
    pub removed_files: Vec<String>,
    pub next_cleanup_at_ms: Option<u64>,
}

/// Settlement changes and the resulting object listing.
#[derive(Debug, Clone, Default)]
pub struct SyncedObjects {
    pub outcome: SettleOutcome,
    pub objects: Vec<ObjectSummary>,
}

/// Deleted record and the output files removed with it.
#[derive(Debug, Clone)]
pub struct DeletedTransaction {
    pub record: TransactionRecord,
    pub removed_files: Vec<String>,
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

        reporter.on_step(
            ExecutionPhase::GenerateProof,
            ExecutionStage::VerifyingInputs.message(),
            &ExecutionStepContext {
                stage: Some(ExecutionStage::VerifyingInputs),
                ..Default::default()
            },
        );
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

        reporter.on_inputs_held(holds.files());
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

        let execution_inputs = resolved_inputs
            .iter()
            .map(|input| input.record.spendable())
            .collect::<Vec<_>>();
        let stage_reporter = reporter.clone();
        let progress = Arc::new(move |stage| {
            let stage = match stage {
                sdk::ExecutionStage::LoadingModule => ExecutionStage::LoadingModule,
                sdk::ExecutionStage::Building => ExecutionStage::Building,
                sdk::ExecutionStage::Mining => ExecutionStage::Mining,
                sdk::ExecutionStage::Solving => ExecutionStage::Solving,
                sdk::ExecutionStage::Proving => ExecutionStage::Proving,
            };
            stage_reporter.on_step(
                ExecutionPhase::GenerateProof,
                stage.message(),
                &ExecutionStepContext {
                    stage: Some(stage),
                    ..Default::default()
                },
            );
        });
        let spendable_outputs = catalog.execute_action_with_progress(
            input.action.clone(),
            grounding_witness,
            execution_inputs,
            &args,
            progress,
        )?;
        if spendable_outputs.objs.len() != action.total_outputs.len() {
            return Err(anyhow!(
                "action {} output mismatch: descriptor expects {}, engine returned {}",
                action.action,
                action.total_outputs.len(),
                spendable_outputs.objs.len()
            ));
        }
        reporter.on_done(ExecutionPhase::GenerateProof);

        let shrink_ctx = ExecutionStepContext {
            stage: Some(ExecutionStage::Shrinking),
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
            owner: owner.to_string(),
            grounding_root,
            grounding_block,
            outputs,
            nullifiers: spendable_outputs.tx.nullifier_hashes()?,
            created: spendable_outputs.tx.created_commitments()?,
            payload_bytes,
            holds,
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
        for input in proved.input_files() {
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
        if read_record(&self.paths, &proved.tx_final)?.is_some() {
            return Err(DriverError::Conflict(format!(
                "transaction {} is already recorded",
                encode_hash_hex(&proved.tx_final)
            ))
            .into());
        }
        for output in &output_files {
            if self.paths.objects_dir.join(output).try_exists()?
                || self.paths.nullified_objects_dir.join(output).try_exists()?
            {
                return Err(
                    DriverError::Conflict(format!("output file already exists: {output}")).into(),
                );
            }
        }

        let record = TransactionRecord::recorded(
            encode_hash_hex(&proved.tx_final),
            proved.action.clone(),
            proved.owner.clone(),
            proved.input_files().to_vec(),
            output_files.clone(),
            encode_hash_hex(&proved.grounding_root),
            proved.grounding_block,
            proved.nullifiers.iter().map(encode_hash_hex).collect(),
            proved.created.iter().map(encode_hash_hex).collect(),
            now_ms(),
        );
        let mut attempted_outputs = Vec::new();
        let result = (|| -> Result<()> {
            for (output, file_name) in proved.outputs.iter().zip(&output_files) {
                attempted_outputs.push(file_name);
                let output_record = ObjectRecord {
                    content_hash: output.obj.commitment(),
                    class: output.class.clone(),
                    status: FileStatus::Unknown,
                    tx_hash: None,
                    obj: output.obj.clone(),
                    tx_final: proved.tx_final,
                };
                write_object_file(&self.paths, &output_record, file_name)?;
            }
            write_payload(&self.paths, &proved.tx_final, &proved.payload_bytes)?;
            write_record(&self.paths, &record)
        })();
        if let Err(mut err) = result {
            for file_name in attempted_outputs {
                let path = self.paths.objects_dir.join(file_name);
                if let Err(cleanup) = fs::remove_file(&path)
                    && cleanup.kind() != std::io::ErrorKind::NotFound
                {
                    err = err.context(format!("failed to roll back output {file_name}: {cleanup}"));
                }
            }
            if let Err(cleanup) = remove_transaction(&self.paths, &proved.tx_final) {
                err = err.context(format!("failed to roll back transaction: {cleanup:#}"));
            }
            return Err(err);
        }
        Ok(record)
    }

    /// Delete a `recorded` transaction and its unknown outputs. Refuse once
    /// submission was attempted: the relayer may have accepted the payload.
    pub fn delete_transaction(&self, tx_final: &Hash) -> Result<DeletedTransaction> {
        let _store = self.store_lock.lock().expect("store lock poisoned");
        let record = read_record(&self.paths, tx_final)?
            .ok_or_else(|| DriverError::TransactionNotFound(encode_hash_hex(tx_final)))?;
        if record.state != TransactionState::Recorded {
            return Err(DriverError::Conflict(format!(
                "transaction {} is {}, only recorded transactions can be deleted",
                record.tx_final, record.state
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
        let removed_files = self.remove_unknown_outputs(&record)?;
        remove_transaction(&self.paths, tx_final)?;
        Ok(DeletedTransaction {
            record,
            removed_files,
        })
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
                    "transaction {} is {state}",
                    record.tx_final
                ))
                .into());
            }
        }
        let head = self
            .deps
            .synchronizer
            .fetch_head(&settings.synchronizer_api_url)?;
        if grounding_aged_out(head.block_number, record.grounding_block) {
            return Err(DriverError::Conflict(format!(
                "transaction {} is grounded at block {}, more than {} blocks behind the synchronizer head {}",
                record.tx_final,
                record.grounding_block,
                txlib::MAX_STATE_ROOT_AGE_BLOCKS,
                head.block_number
            ))
            .into());
        }

        let payload_bytes = crate::transactions::read_payload(&self.paths, tx_final)?;
        let (record, attempted) = self.update_record(tx_final, |record| {
            if record.state != TransactionState::Recorded {
                return Ok(false);
            }
            record.submit_attempted_at_ms.get_or_insert_with(now_ms);
            Ok(true)
        })?;
        if !attempted {
            return Ok(record);
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
    pub fn sync_objects(&self, query: Option<&ObjectQuery>) -> Result<SyncedObjects> {
        let (outcome, objects) = self.settle_with(true)?;
        Ok(SyncedObjects {
            outcome,
            objects: objects
                .into_iter()
                .filter(|summary| query.is_none_or(|query| matches_query(summary, query)))
                .collect(),
        })
    }

    /// With `all_objects`, query and summarize every object; otherwise,
    /// summarize only files changed by settlement.
    fn settle_with(&self, all_objects: bool) -> Result<(SettleOutcome, Vec<ObjectSummary>)> {
        let settings = self.load_settings()?;
        let records = load_records(&self.paths)?;
        let queried_transactions = records
            .iter()
            .filter(|record| record.is_open())
            .map(|record| record.tx_final.clone())
            .collect::<HashSet<_>>();
        let queried_membership = records
            .iter()
            .filter(|record| record.needs_membership())
            .map(|record| record.tx_final.clone())
            .collect::<HashSet<_>>();
        let mut commitments = HashSet::new();
        let mut nullifiers = HashSet::new();
        for record in records.iter().filter(|record| record.needs_membership()) {
            commitments.extend(decode_hashes(&record.created)?);
            nullifiers.extend(decode_hashes(&record.nullifiers)?);
        }
        if all_objects {
            for entry in load_object_files(&self.paths)? {
                commitments.insert(entry.record.obj.commitment());
                if let Ok(nullifier) = object_nullifier_hash(&entry.record.obj) {
                    nullifiers.insert(nullifier);
                }
            }
        }
        let membership = if !commitments.is_empty()
            || !nullifiers.is_empty()
            || !queried_membership.is_empty()
        {
            Some(self.deps.synchronizer.fetch_membership_with_nullifiers(
                &settings.synchronizer_api_url,
                &commitments.into_iter().collect::<Vec<_>>(),
                &nullifiers.into_iter().collect::<Vec<_>>(),
            )?)
        } else {
            None
        };
        let expiry_block = match membership.as_ref() {
            Some(membership) => Some(membership.block_number),
            None if !queried_transactions.is_empty() => Some(
                self.deps
                    .synchronizer
                    .fetch_head(&settings.synchronizer_api_url)?
                    .block_number,
            ),
            None => None,
        };
        let Some(expiry_block) = expiry_block else {
            let outcome = SettleOutcome {
                open_records: records.iter().filter(|record| record.is_open()).count(),
                unattempted_records: records
                    .iter()
                    .filter(|record| record.is_open() && !record.needs_membership())
                    .count(),
                ..SettleOutcome::default()
            };
            let listing = if all_objects {
                self.summaries(&load_object_files(&self.paths)?, &records)
            } else {
                Vec::new()
            };
            return Ok((outcome, listing));
        };

        let mut relayer_jobs = HashMap::new();
        for record in &records {
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
        for record in records
            .iter_mut()
            .filter(|record| record.is_open() && queried_transactions.contains(&record.tx_final))
        {
            let record_membership = membership
                .as_ref()
                .filter(|_| queried_membership.contains(&record.tx_final));
            if record.needs_membership() && record_membership.is_none() {
                continue;
            }
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
            let landed = if let Some(membership) = record_membership {
                let created = decode_hashes(&record.created)?;
                let spent = decode_hashes(&record.nullifiers)?;
                created
                    .iter()
                    .all(|commitment| membership.created_objects.contains(commitment))
                    && spent
                        .iter()
                        .all(|nullifier| membership.on_chain_nullifiers.contains(nullifier))
            } else {
                false
            };
            if landed {
                record.state = TransactionState::Landed;
                record.settled_at_ms = Some(settled_at_ms);
            } else if grounding_aged_out(expiry_block, record.grounding_block) {
                record.state = TransactionState::Expired;
                record.reason = Some(format!(
                    "grounding block {} is more than {} blocks behind the synchronizer head {}",
                    record.grounding_block,
                    txlib::MAX_STATE_ROOT_AGE_BLOCKS,
                    expiry_block
                ));
                record.settled_at_ms = Some(settled_at_ms);
                let removed = self.remove_unknown_outputs(record)?;
                outcome.removed_files.extend(removed);
            }
            if *record != before {
                write_record(&self.paths, record)?;
                outcome.records.push(record.clone());
            }
        }
        outcome.open_records = records.iter().filter(|record| record.is_open()).count();
        outcome.unattempted_records = records
            .iter()
            .filter(|record| record.is_open() && !record.needs_membership())
            .count();

        let mut entries = load_object_files(&self.paths)?;
        let record_index = RecordIndex::new(&records);
        let mut changed_files = HashSet::new();
        for entry in entries.iter_mut() {
            let commitment = entry.record.obj.commitment();
            let nullifier = object_nullifier_hash(&entry.record.obj).ok();
            let status = match entry.record.status {
                FileStatus::Nullified => FileStatus::Nullified,
                _ if membership.as_ref().is_some_and(|membership| {
                    nullifier.is_some_and(|n| membership.on_chain_nullifiers.contains(&n))
                }) =>
                {
                    FileStatus::Nullified
                }
                FileStatus::Unknown
                    if membership.as_ref().is_some_and(|membership| {
                        membership.created_objects.contains(&commitment)
                    }) =>
                {
                    FileStatus::Live
                }
                status => status,
            };
            let tx_hash = record_index
                .by_tx_final
                .get(&entry.record.tx_final)
                .and_then(|record| record.tx_hash.clone())
                .or_else(|| entry.record.tx_hash.clone());
            if entry.record.status == status && entry.record.tx_hash == tx_hash {
                continue;
            }
            entry.record.status = status;
            entry.record.tx_hash = tx_hash;
            write_object_file(&self.paths, &entry.record, &entry.file_name)?;
            changed_files.insert(entry.file_name.clone());
        }
        for record in &outcome.records {
            if record.state.is_terminal() {
                changed_files.extend(record.output_files.iter().cloned());
                changed_files.extend(record.input_files.iter().cloned());
            }
        }
        let summaries = self.summaries(
            entries
                .iter()
                .filter(|entry| all_objects || changed_files.contains(&entry.file_name)),
            &records,
        );
        let listing = if all_objects {
            outcome.objects = summaries
                .iter()
                .filter(|summary| changed_files.contains(&summary.file_name))
                .cloned()
                .collect();
            summaries
        } else {
            outcome.objects = summaries;
            Vec::new()
        };
        Ok((outcome, listing))
    }

    pub fn prune_transactions(&self, current_time_ms: u64) -> Result<PrunedTransactions> {
        let _store = self.store_lock.lock().expect("store lock poisoned");
        let records = load_records(&self.paths)?;
        let mut outcome = PrunedTransactions::default();
        let mut changed_files = HashSet::new();
        for record in records.iter().filter(|record| record.state.is_terminal()) {
            if let Err(err) =
                self.persist_settled_objects(record, &mut changed_files, &mut outcome.removed_files)
            {
                log::warn!("retaining transaction {}: {err:#}", record.tx_final);
                let retry_at = current_time_ms.saturating_add(60_000);
                outcome.next_cleanup_at_ms = Some(
                    outcome
                        .next_cleanup_at_ms
                        .map_or(retry_at, |next| next.min(retry_at)),
                );
                continue;
            }
            let cleanup_at = record
                .settled_at_ms
                .unwrap_or(record.recorded_at_ms)
                .saturating_add(TRANSACTION_RETENTION_MS);
            if current_time_ms >= cleanup_at {
                match remove_transaction(&self.paths, &record_tx_final(record)?) {
                    Ok(()) => outcome.tx_finals.push(record.tx_final.clone()),
                    Err(err) => {
                        log::warn!("could not prune transaction {}: {err:#}", record.tx_final);
                        let retry_at = current_time_ms.saturating_add(60_000);
                        outcome.next_cleanup_at_ms = Some(
                            outcome
                                .next_cleanup_at_ms
                                .map_or(retry_at, |next| next.min(retry_at)),
                        );
                    }
                }
            } else {
                outcome.next_cleanup_at_ms = Some(
                    outcome
                        .next_cleanup_at_ms
                        .map_or(cleanup_at, |next| next.min(cleanup_at)),
                );
            }
        }
        if !changed_files.is_empty() {
            let remaining = records
                .into_iter()
                .filter(|record| !outcome.tx_finals.contains(&record.tx_final))
                .collect::<Vec<_>>();
            outcome.objects = self.summaries(
                load_object_files(&self.paths)?
                    .iter()
                    .filter(|entry| changed_files.contains(&entry.file_name)),
                &remaining,
            );
        }
        Ok(outcome)
    }

    fn persist_settled_objects(
        &self,
        record: &TransactionRecord,
        changed_files: &mut HashSet<String>,
        removed_files: &mut Vec<String>,
    ) -> Result<()> {
        if record.state == TransactionState::Expired {
            for file_name in &record.output_files {
                let path = self.paths.objects_dir.join(file_name);
                if path.try_exists()? {
                    crate::object_record::parse_object_record_file(&path)?;
                }
            }
            removed_files.extend(self.remove_unknown_outputs(record)?);
            return Ok(());
        }
        let producing_tx_final = record_tx_final(record)?;
        for (files, output) in [(&record.input_files, false), (&record.output_files, true)] {
            for file_name in files {
                let live_path = self.paths.objects_dir.join(file_name);
                let nullified_path = self.paths.nullified_objects_dir.join(file_name);
                let path = if live_path.try_exists()? {
                    live_path
                } else {
                    nullified_path
                };
                if !path.try_exists()? {
                    continue;
                }
                let mut stored = crate::object_record::parse_object_record_file(&path)?;
                if output && stored.tx_final != producing_tx_final {
                    continue;
                }
                let mut changed = false;
                if output {
                    if stored.status == FileStatus::Unknown {
                        stored.status = FileStatus::Live;
                        changed = true;
                    }
                    if let Some(tx_hash) = &record.tx_hash
                        && stored.tx_hash.as_ref() != Some(tx_hash)
                    {
                        stored.tx_hash = Some(tx_hash.clone());
                        changed = true;
                    }
                } else if stored.status != FileStatus::Nullified {
                    stored.status = FileStatus::Nullified;
                    changed = true;
                }
                if changed {
                    write_object_file(&self.paths, &stored, file_name)?;
                    changed_files.insert(file_name.clone());
                }
            }
        }
        Ok(())
    }

    pub fn list_transactions(&self) -> Result<Vec<TransactionRecord>> {
        load_records(&self.paths)
    }

    pub fn get_transaction(&self, tx_final: &Hash) -> Result<TransactionRecord> {
        read_record(&self.paths, tx_final)?
            .ok_or_else(|| DriverError::TransactionNotFound(encode_hash_hex(tx_final)).into())
    }

    /// Prove, record, and submit under owner `execute`, without waiting for landing.
    pub fn execute(&self, input: ExecuteActionInput) -> Result<TransactionRecord> {
        let proved = self.prove(input, "execute", &crate::types::NoopExecutionReporter)?;
        let record = self.record(proved)?;
        self.submit(&record_tx_final(&record)?)
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

    /// Remove unknown outputs still owned by this record. Caller holds the store lock.
    fn remove_unknown_outputs(&self, record: &TransactionRecord) -> Result<Vec<String>> {
        let tx_final = record_tx_final(record)?;
        let mut removed = Vec::new();
        for file_name in &record.output_files {
            let path = self.paths.objects_dir.join(file_name);
            let Ok(stored) = crate::object_record::parse_object_record_file(&path) else {
                continue;
            };
            if stored.status == FileStatus::Unknown && stored.tx_final == tx_final {
                fs::remove_file(&path)
                    .with_context(|| format!("failed to remove output {file_name}"))?;
                removed.push(file_name.clone());
            }
        }
        Ok(removed)
    }
}

pub(crate) struct RecordIndex<'a> {
    pub(crate) by_tx_final: HashMap<Hash, &'a TransactionRecord>,
    /// Input filenames mapped to the owner of the open transaction spending them.
    pub(crate) locked_by: HashMap<&'a str, &'a str>,
}

impl<'a> RecordIndex<'a> {
    pub(crate) fn new(records: &'a [TransactionRecord]) -> Self {
        let by_tx_final = records
            .iter()
            .filter_map(|record| {
                record_tx_final(record)
                    .ok()
                    .map(|tx_final| (tx_final, record))
            })
            .collect();
        let locked_by = records
            .iter()
            .filter(|record| record.is_open())
            .flat_map(|record| {
                record
                    .input_files
                    .iter()
                    .map(move |file| (file.as_str(), record.owner.as_str()))
            })
            .collect();
        Self {
            by_tx_final,
            locked_by,
        }
    }
}
