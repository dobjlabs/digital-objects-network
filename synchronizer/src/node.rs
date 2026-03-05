use std::{collections::HashMap, sync::Arc, time::Duration};

use crate::clients::beacon::{
    self,
    types::{Blob, BlockHeader, BlockId},
    BeaconClient,
};

use alloy::{
    consensus::Transaction,
    eips::{self as alloy_eips, eip4844::kzg_to_versioned_hash},
    network as alloy_network,
    primitives::B256,
    providers as alloy_provider,
    transports::http::reqwest,
};
use alloy_network::Ethereum;
use alloy_provider::{Provider, RootProvider};
use anyhow::{anyhow, Context, Result};
use backoff::ExponentialBackoffBuilder;
use chrono::{DateTime, Utc};
use tracing::{debug, info, trace};

use crate::blob::bytes_from_simple_blob;
use crate::config::AppConfig;
use crate::state_machine::{SlotDelta, StateMachine};
use crate::sync_db::{JournalOp, SlotJournal, SyncDb};

/// Runtime integration layer that connects network inputs (beacon/execution),
/// pure state derivation (`StateMachine`), and sync metadata (`SyncDb`).
pub struct Node {
    pub beacon_cli: BeaconClient,
    pub rpc_cli: RootProvider,
    pub config: AppConfig,
    pub state_machine: Arc<StateMachine>,
    pub sync_db: Arc<SyncDb>,
}

struct SlotContext {
    slot: u32,
    beacon_block_root: B256,
    parent_root: B256,
    execution_block_hash: B256,
    execution_block_number: u32,
    execution_timestamp: u64,
    has_blob_commitments: bool,
}

/// Fully-derived result for one slot, ready to be journaled/applied/finalized.
pub struct ProcessedSlot {
    pub slot: u32,
    pub block_root: B256,
    pub parent_root: B256,
    pub block_number: Option<u32>,
    pub is_empty: bool,
    pub delta: SlotDelta,
}

impl Node {
    /// Construct network clients and bind shared state/sync stores.
    pub async fn new(
        cfg: AppConfig,
        state_machine: Arc<StateMachine>,
        sync_db: Arc<SyncDb>,
    ) -> Result<Self> {
        let http_cli = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()?;

        let exp_backoff = Some(ExponentialBackoffBuilder::default().build());
        let beacon_cli_cfg = beacon::Config {
            base_url: cfg.beacon_url.clone(),
            exp_backoff,
        };
        let beacon_cli = BeaconClient::try_with_client(http_cli, beacon_cli_cfg)?;
        let rpc_cli = RootProvider::<Ethereum>::new_http(cfg.rpc_url.parse()?);

        Ok(Self {
            beacon_cli,
            rpc_cli,
            config: cfg,
            state_machine,
            sync_db,
        })
    }

    pub async fn last_processed_slot(&self) -> Result<Option<u32>> {
        self.sync_db.last_processed_slot().await
    }

    pub async fn slot_root(&self, slot: u32) -> Result<Option<B256>> {
        self.sync_db.slot_root(slot).await
    }

    /// Rewind to `keep_slot` by staging rollback in Postgres, deleting affected keys in RocksDB,
    /// and finalizing rollback metadata.
    pub async fn rollback_to_slot(&self, keep_slot: Option<u32>) -> Result<()> {
        let journals = self.sync_db.rollback_to_slot(keep_slot).await?;
        self.state_machine.rollback_journals(&journals)?;
        let rollback_slots: Vec<u32> = journals.iter().map(|j| j.slot).collect();
        self.sync_db.complete_rollback(&rollback_slots).await?;
        Ok(())
    }

    /// Recover incomplete apply/rollback operations after crash or shutdown.
    ///
    /// Recovery source of truth is Postgres journal state (`pending_recoveries`).
    pub async fn recover_pending(&self) -> Result<()> {
        let recoveries = self.sync_db.pending_recoveries().await?;
        if recoveries.is_empty() {
            return Ok(());
        }

        for recovery in recoveries {
            match recovery.op {
                JournalOp::Apply => {
                    self.state_machine.apply_journal(&recovery.journal)?;
                    self.sync_db
                        .finalize_slot_applied(recovery.slot, recovery.block_number)
                        .await?;
                }
                JournalOp::Rollback => {
                    self.state_machine
                        .rollback_journals(std::slice::from_ref(&recovery.journal))?;
                    self.sync_db.complete_rollback(&[recovery.slot]).await?;
                }
            }
        }
        self.state_machine.reload_from_db()?;
        Ok(())
    }

    /// Fetch beacon blob sidecars for a slot and retain only requested versioned hashes.
    async fn get_blobs(&self, slot: u32, versioned_hashes: &[B256]) -> Result<HashMap<B256, Blob>> {
        let blobs = self.beacon_cli.get_blobs(slot.into()).await?;
        debug!(slot, blob_count = blobs.len(), "Fetched blobs from beacon");
        let blobs: HashMap<_, _> = blobs
            .into_iter()
            .filter_map(|blob| {
                let versioned_hash = kzg_to_versioned_hash(blob.kzg_commitment.as_ref());
                versioned_hashes
                    .contains(&versioned_hash)
                    .then_some((versioned_hash, blob))
            })
            .collect();

        for vh in versioned_hashes {
            if !blobs.contains_key(vh) {
                return Err(anyhow!(
                    "Missing requested blob in beacon response: slot={slot}, versioned_hash={vh}"
                ));
            }
        }

        Ok(blobs)
    }

    /// Resolve consensus+execution context required to derive this slot.
    ///
    /// Returns `None` for slots without an execution payload.
    async fn build_slot_context(
        &self,
        beacon_block_header: &BlockHeader,
    ) -> Result<Option<SlotContext>> {
        let beacon_block_root = beacon_block_header.root;
        let slot = beacon_block_header.slot;

        let beacon_block = match self
            .beacon_cli
            .get_block(BlockId::Hash(beacon_block_root))
            .await?
        {
            Some(block) => block,
            None => {
                debug!(slot, "No consensus block for slot");
                return Ok(None);
            }
        };

        let execution_payload = match beacon_block.execution_payload.as_ref() {
            Some(payload) => payload,
            None => {
                debug!(slot, "Consensus block has no execution payload");
                return Ok(None);
            }
        };

        let has_blob_commitments = beacon_block
            .blob_kzg_commitments
            .as_ref()
            .is_some_and(|commitments| !commitments.is_empty());

        Ok(Some(SlotContext {
            slot,
            beacon_block_root,
            parent_root: beacon_block.parent_root,
            execution_block_hash: execution_payload.block_hash,
            execution_block_number: execution_payload.block_number,
            execution_timestamp: execution_payload.timestamp,
            has_blob_commitments,
        }))
    }

    /// Derive the full per-slot update from beacon/execution data without mutating live memory.
    pub async fn derive_slot_update(
        &self,
        beacon_block_header: &BlockHeader,
    ) -> Result<ProcessedSlot> {
        let Some(slot_ctx) = self.build_slot_context(beacon_block_header).await? else {
            return Ok(ProcessedSlot {
                slot: beacon_block_header.slot,
                block_root: beacon_block_header.root,
                parent_root: beacon_block_header.parent_root,
                block_number: None,
                is_empty: true,
                delta: SlotDelta::default(),
            });
        };

        debug!(
            slot = slot_ctx.slot,
            execution_block_hash = ?slot_ctx.execution_block_hash,
            execution_block_number = slot_ctx.execution_block_number,
            "Resolved execution payload for slot"
        );
        info!(
            "Processing slot {} from {}",
            slot_ctx.slot,
            DateTime::<Utc>::from_timestamp_secs(slot_ctx.execution_timestamp as i64)
                .unwrap_or_default(),
        );
        self.state_machine.log_current_state()?;

        let block_number = slot_ctx.execution_block_number;

        if !slot_ctx.has_blob_commitments {
            debug!(slot = slot_ctx.slot, "Slot has no blob commitments");
            let delta = self
                .state_machine
                .derive_slot_delta(slot_ctx.slot, block_number, &[])?;
            return Ok(ProcessedSlot {
                slot: slot_ctx.slot,
                block_root: slot_ctx.beacon_block_root,
                parent_root: slot_ctx.parent_root,
                block_number: Some(block_number),
                is_empty: false,
                delta,
            });
        }

        let mut blob_payloads = Vec::new();

        let execution_block_id =
            alloy_eips::eip1898::BlockId::Hash(slot_ctx.execution_block_hash.into());
        let execution_block = self
            .rpc_cli
            .get_block(execution_block_id)
            .full()
            .await?
            .with_context(|| {
                format!(
                    "Execution block {} not found",
                    slot_ctx.execution_block_hash
                )
            })?;

        let indexed_do_blob_txs: Vec<_> = match execution_block.transactions.as_transactions() {
            Some(txs) => txs
                .iter()
                .enumerate()
                .filter(|(_index, tx)| {
                    tx.inner.blob_versioned_hashes().is_some()
                        && tx.as_recovered().to() == Some(self.config.to_address)
                })
                .collect(),
            None => {
                return Err(anyhow!(
                    "Consensus block {} has blobs but the execution block doesn't have txs",
                    slot_ctx.beacon_block_root
                ));
            }
        };

        if !indexed_do_blob_txs.is_empty() {
            let blob_versioned_hashes: Vec<B256> = indexed_do_blob_txs
                .iter()
                .flat_map(|(_, tx)| {
                    tx.as_recovered()
                        .blob_versioned_hashes()
                        .expect("tx has blobs")
                })
                .cloned()
                .collect();
            let blobs = self
                .get_blobs(slot_ctx.slot, &blob_versioned_hashes)
                .await?;

            for (_tx_index, tx) in indexed_do_blob_txs {
                let tx = tx.as_recovered();
                let hash = tx.hash();
                let from = tx.signer();
                let to = tx.to();
                let tx_blobs: Vec<_> = tx
                    .blob_versioned_hashes()
                    .expect("tx has blobs")
                    .iter()
                    .map(|vh| &blobs[vh])
                    .collect();
                trace!(?hash, ?from, ?to);

                for blob in tx_blobs.iter() {
                    let bytes = bytes_from_simple_blob(blob.blob.inner()).with_context(|| {
                        format!(
                            "Invalid byte encoding in blob at slot {}, blob_index {}",
                            slot_ctx.slot, blob.index
                        )
                    })?;
                    blob_payloads.push(bytes);
                    info!(
                        slot = slot_ctx.slot,
                        blob_index = blob.index,
                        tx_hash = ?hash,
                        "Decoded target blob"
                    );
                }
            }
        }

        let delta =
            self.state_machine
                .derive_slot_delta(slot_ctx.slot, block_number, &blob_payloads)?;

        Ok(ProcessedSlot {
            slot: slot_ctx.slot,
            block_root: slot_ctx.beacon_block_root,
            parent_root: slot_ctx.parent_root,
            block_number: Some(block_number),
            is_empty: false,
            delta,
        })
    }

    /// Apply a derived slot delta to RocksDB.
    pub fn apply_delta_to_db(&self, delta: &SlotDelta) -> Result<()> {
        self.state_machine.apply_delta_to_db(delta)
    }

    /// Apply a derived slot delta to in-memory state.
    ///
    /// Called only after finalize in the sync loop.
    pub fn apply_delta_to_memory(&self, delta: &SlotDelta) -> Result<()> {
        self.state_machine.apply_delta_to_memory(delta)
    }

    /// Stage canonical slot metadata and slot journal in Postgres as `pending`.
    pub async fn save_pending_slot(&self, processed: &ProcessedSlot) -> Result<()> {
        let journal = SlotJournal {
            slot: processed.slot,
            tx_hashes: processed.delta.tx_hashes.clone(),
            nullifiers: processed.delta.nullifiers.clone(),
            gsr_block_numbers: processed.delta.gsr_block_numbers.clone(),
            gsr_hashes: processed.delta.gsr_hashes.clone(),
        };

        self.sync_db
            .save_pending_slot(
                processed.slot,
                if processed.is_empty {
                    None
                } else {
                    Some(processed.block_root)
                },
                if processed.is_empty {
                    None
                } else {
                    Some(processed.parent_root)
                },
                processed.block_number,
                processed.is_empty,
                &journal,
            )
            .await
    }

    /// Mark a pending slot applied and advance the sync cursor in Postgres.
    pub async fn finalize_slot_applied(&self, slot: u32, block_number: Option<u32>) -> Result<()> {
        self.sync_db.finalize_slot_applied(slot, block_number).await
    }

    /// Reload in-memory state from the persisted app database.
    pub fn reload_from_db(&self) -> Result<()> {
        self.state_machine.reload_from_db()
    }
}
