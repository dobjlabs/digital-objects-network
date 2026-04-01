use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

use crate::clients::beacon::{
    self,
    types::{Blob, Block, BlockHeader, BlockId, Spec},
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
use pod2::middleware::Hash;
use tracing::{debug, info, trace, warn};

use crate::config::AppConfig;
use crate::head::CanonicalHead;
use crate::state_machine::{StateMachine, MAX_GSR_AGE_BLOCKS};
use crate::sync_db::{CommittedSlotRecord, SyncDb};

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

/// Fully-derived result for one slot, ready to be committed.
pub struct ProcessedSlot {
    pub slot: u32,
    pub block_root: B256,
    pub parent_root: B256,
    pub block_number: Option<u32>,
    pub is_empty: bool,
    pub new_head: CanonicalHead,
}

impl ProcessedSlot {
    pub(crate) fn empty(
        slot: u32,
        block_root: B256,
        parent_root: B256,
        new_head: CanonicalHead,
    ) -> Self {
        Self {
            slot,
            block_root,
            parent_root,
            block_number: None,
            is_empty: true,
            new_head,
        }
    }

    fn present(
        slot: u32,
        block_root: B256,
        parent_root: B256,
        block_number: u32,
        new_head: CanonicalHead,
    ) -> Self {
        Self {
            slot,
            block_root,
            parent_root,
            block_number: Some(block_number),
            is_empty: false,
            new_head,
        }
    }

    fn canonical_block_root(&self) -> Option<B256> {
        (!self.is_empty).then_some(self.block_root)
    }

    fn canonical_parent_root(&self) -> Option<B256> {
        (!self.is_empty).then_some(self.parent_root)
    }

    fn canonical_current_gsr(&self) -> Option<Hash> {
        if self.is_empty {
            None
        } else {
            self.new_head.metadata.current_gsr
        }
    }
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

    pub async fn last_processed_slot(&self) -> Result<u32> {
        self.sync_db.last_processed_slot().await
    }

    pub async fn slot_root(&self, slot: u32) -> Result<Option<B256>> {
        self.sync_db.slot_root(slot).await
    }

    pub async fn current_head(&self) -> Result<CanonicalHead> {
        self.sync_db.current_head().await
    }

    /// Rewind to `keep_slot` by deleting later canonical slot rows.
    pub async fn rollback_to_slot(&self, keep_slot: u32) -> Result<()> {
        self.sync_db.rollback_to_slot(keep_slot).await
    }

    async fn retry_rpc<T, Op, Fut>(&self, operation: &str, target: String, mut op: Op) -> Result<T>
    where
        Op: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let mut retry = 0;

        loop {
            match op().await {
                Ok(value) => return Ok(value),
                Err(err) => {
                    if retry >= self.config.rpc_retries {
                        return Err(err).with_context(|| {
                            format!(
                                "RPC operation `{operation}` failed for {target} after {} retries",
                                self.config.rpc_retries
                            )
                        });
                    }

                    retry += 1;
                    warn!(
                        operation,
                        target = %target,
                        retry,
                        max_retries = self.config.rpc_retries,
                        retry_delay_ms = self.config.rpc_retry_delay.as_millis() as u64,
                        ?err,
                        "RPC operation failed; retrying"
                    );
                    tokio::time::sleep(self.config.rpc_retry_delay).await;
                }
            }
        }
    }

    pub(crate) async fn get_beacon_spec_with_retry(&self) -> Result<Spec> {
        self.retry_rpc("beacon spec", "config/spec".to_string(), || async {
            Ok(self.beacon_cli.get_spec().await?)
        })
        .await
    }

    pub(crate) async fn get_beacon_head_header_with_retry(&self) -> Result<BlockHeader> {
        self.retry_rpc("beacon head header", "head".to_string(), || async {
            self.beacon_cli
                .get_block_header(BlockId::Head)
                .await?
                .ok_or_else(|| anyhow!("Beacon head header not found"))
        })
        .await
    }

    pub(crate) async fn get_beacon_slot_header_with_retry(
        &self,
        slot: u32,
    ) -> Result<Option<BlockHeader>> {
        self.retry_rpc("beacon slot header", format!("slot {slot}"), || async {
            Ok(self
                .beacon_cli
                .get_block_header(BlockId::Slot(slot))
                .await?)
        })
        .await
    }

    /// Fetch beacon blob sidecars for a slot and retain only requested versioned hashes.
    async fn get_blobs(&self, slot: u32, versioned_hashes: &[B256]) -> Result<HashMap<B256, Blob>> {
        let blobs = self
            .retry_rpc("beacon blob sidecars", format!("slot {slot}"), || async {
                let blobs = self.beacon_cli.get_blobs(slot.into()).await?;
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
            })
            .await?;
        debug!(slot, blob_count = blobs.len(), "Fetched blobs from beacon");

        Ok(blobs)
    }

    pub(crate) async fn get_beacon_block_by_hash_with_retry(
        &self,
        slot: u32,
        beacon_block_root: B256,
    ) -> Result<Block> {
        self.retry_rpc(
            "beacon block",
            format!("slot {slot}, block_root {beacon_block_root}"),
            || async {
                self.beacon_cli
                    .get_block(BlockId::Hash(beacon_block_root))
                    .await?
                    .ok_or_else(|| {
                        anyhow!(
                            "Beacon header exists for slot {slot}, but full beacon block {beacon_block_root} was not found"
                        )
                    })
            },
        )
        .await
    }

    pub(crate) async fn load_committed_slot_record(
        &self,
        slot: u32,
    ) -> Result<CommittedSlotRecord> {
        let Some(header) = self.get_beacon_slot_header_with_retry(slot).await? else {
            return Ok(CommittedSlotRecord {
                slot,
                block_root: None,
                parent_root: None,
                block_number: None,
                current_gsr: None,
                is_empty: true,
            });
        };

        let block = self
            .get_beacon_block_by_hash_with_retry(slot, header.root)
            .await?;
        let execution_payload = block.execution_payload.as_ref().ok_or_else(|| {
            anyhow!(
                "Beacon block {} for slot {slot} had no execution payload",
                header.root
            )
        })?;

        Ok(CommittedSlotRecord {
            slot,
            block_root: Some(header.root),
            parent_root: Some(block.parent_root),
            block_number: Some(execution_payload.block_number),
            current_gsr: None,
            is_empty: false,
        })
    }

    /// Resolve the full consensus+execution context required to derive a present slot.
    ///
    /// The caller already has a canonical beacon header for this slot, so failure to load the
    /// corresponding full beacon block or execution payload is treated as an error rather than as
    /// an empty slot. This forces the sync loop to retry instead of silently advancing past a real
    /// slot when the beacon provider is temporarily inconsistent.
    async fn build_slot_context(&self, beacon_block_header: &BlockHeader) -> Result<SlotContext> {
        let beacon_block_root = beacon_block_header.root;
        let slot = beacon_block_header.slot;

        let beacon_block = self
            .get_beacon_block_by_hash_with_retry(slot, beacon_block_root)
            .await?;

        let execution_payload = beacon_block.execution_payload.as_ref().ok_or_else(|| {
            anyhow!("Beacon block {beacon_block_root} for slot {slot} had no execution payload")
        })?;

        let has_blob_commitments = beacon_block
            .blob_kzg_commitments
            .as_ref()
            .is_some_and(|commitments| !commitments.is_empty());

        Ok(SlotContext {
            slot,
            beacon_block_root,
            parent_root: beacon_block.parent_root,
            execution_block_hash: execution_payload.block_hash,
            execution_block_number: execution_payload.block_number,
            execution_timestamp: execution_payload.timestamp,
            has_blob_commitments,
        })
    }

    /// Derive the full per-slot update from beacon/execution data and return it for commit.
    pub async fn derive_slot_update(
        &self,
        beacon_block_header: &BlockHeader,
    ) -> Result<ProcessedSlot> {
        let base_head = self.sync_db.current_head().await?;
        let slot_ctx = self.build_slot_context(beacon_block_header).await?;

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
        self.state_machine.log_current_state(base_head);

        let block_number = slot_ctx.execution_block_number;
        let min_block_number = base_head
            .metadata
            .current_block_number
            .map(|n| n.saturating_sub(MAX_GSR_AGE_BLOCKS as u32));
        let recent_gsrs = self.sync_db.recent_gsrs(min_block_number).await?;

        if !slot_ctx.has_blob_commitments {
            debug!(slot = slot_ctx.slot, "Slot has no blob commitments");
            let new_head = self.state_machine.derive_slot_head(
                base_head,
                recent_gsrs,
                slot_ctx.slot,
                block_number,
                &[],
            )?;
            return Ok(ProcessedSlot::present(
                slot_ctx.slot,
                slot_ctx.beacon_block_root,
                slot_ctx.parent_root,
                block_number,
                new_head,
            ));
        }

        let mut blob_payloads = Vec::new();

        let execution_block = self
            .retry_rpc(
                "execution block",
                format!(
                    "slot {}, block_hash {}",
                    slot_ctx.slot, slot_ctx.execution_block_hash
                ),
                || async {
                    let execution_block_id =
                        alloy_eips::eip1898::BlockId::Hash(slot_ctx.execution_block_hash.into());
                    self.rpc_cli
                        .get_block(execution_block_id)
                        .full()
                        .await?
                        .ok_or_else(|| {
                            anyhow!(
                                "Execution block {} not found",
                                slot_ctx.execution_block_hash
                            )
                        })
                },
            )
            .await?;

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

        if indexed_do_blob_txs.is_empty() {
            debug!(
                slot = slot_ctx.slot,
                execution_block_number = block_number,
                to_address = ?self.config.to_address,
                "No matching target blob transactions in execution block"
            );
            let new_head = self.state_machine.derive_slot_head(
                base_head,
                recent_gsrs,
                slot_ctx.slot,
                block_number,
                &[],
            )?;
            return Ok(ProcessedSlot::present(
                slot_ctx.slot,
                slot_ctx.beacon_block_root,
                slot_ctx.parent_root,
                block_number,
                new_head,
            ));
        }

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
                let bytes =
                    common::blob::decode_simple_blob(blob.blob.inner()).with_context(|| {
                        format!(
                            "Invalid byte encoding in blob at slot {}, blob_index {}",
                            slot_ctx.slot, blob.index
                        )
                    })?;
                blob_payloads.push((blob.index, bytes));
                info!(
                    slot = slot_ctx.slot,
                    blob_index = blob.index,
                    tx_hash = ?hash,
                    "Decoded target blob"
                );
            }
        }

        let new_head = self.state_machine.derive_slot_head(
            base_head,
            recent_gsrs,
            slot_ctx.slot,
            block_number,
            &blob_payloads,
        )?;

        Ok(ProcessedSlot::present(
            slot_ctx.slot,
            slot_ctx.beacon_block_root,
            slot_ctx.parent_root,
            block_number,
            new_head,
        ))
    }

    /// Commit one derived slot to Postgres as the new canonical head.
    pub async fn commit_slot(&self, processed: &ProcessedSlot) -> Result<()> {
        let slot = CommittedSlotRecord {
            slot: processed.slot,
            block_root: processed.canonical_block_root(),
            parent_root: processed.canonical_parent_root(),
            block_number: processed.block_number,
            current_gsr: processed.canonical_current_gsr(),
            is_empty: processed.is_empty,
        };

        self.sync_db.commit_slot(&slot, &processed.new_head).await
    }
}
