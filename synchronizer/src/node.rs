use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

use eth_clients::beacon::{
    self,
    types::{BlobSidecar, Block, BlockHeader, BlockId, Spec},
    BeaconClient,
};

use alloy::{
    consensus::Transaction,
    eips::{self as alloy_eips, eip4844::kzg_to_versioned_hash},
    network as alloy_network,
    primitives::B256,
    providers as alloy_provider,
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
use crate::state_machine::{DerivedSlot, StateMachine, MAX_GSR_AGE_BLOCKS};
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

/// Outcome of processing one beacon slot, ready to be committed.
pub enum ProcessedSlot {
    /// Beacon produced no block for the slot. With no execution block there is
    /// nothing to derive against, so the previous canonical head is carried
    /// forward unchanged, committed under the new slot number to keep canonical
    /// slot history contiguous.
    Missing {
        slot: u32,
        carried_head: CanonicalHead,
    },
    /// Beacon produced a block and the state machine derived the slot against
    /// it (deriving a fresh GSR even when the block carries no usable blobs).
    Present {
        slot: u32,
        block_root: B256,
        parent_root: B256,
        block_number: u32,
        derived: DerivedSlot,
    },
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

    /// Rewind to `keep_slot` by deleting later canonical slot rows; the created
    /// index rows those slots added are pruned in the same Postgres transaction.
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
    async fn get_blobs(
        &self,
        slot: u32,
        versioned_hashes: &[B256],
    ) -> Result<HashMap<B256, BlobSidecar>> {
        let blobs = self
            .retry_rpc("beacon blob sidecars", format!("slot {slot}"), || async {
                let blobs = self.beacon_cli.get_blob_sidecars(slot.into()).await?;
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
            return Ok(CommittedSlotRecord::empty(slot));
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

    /// Build a `SlotContext` from a beacon header and its full block.
    ///
    /// Failure to extract the execution payload is treated as an error rather than as an empty
    /// slot. This forces the sync loop to retry instead of silently advancing past a real slot
    /// when the beacon provider is temporarily inconsistent.
    fn slot_context_from_block(
        beacon_block_header: &BlockHeader,
        beacon_block: &Block,
    ) -> Result<SlotContext> {
        let slot = beacon_block_header.slot;
        let beacon_block_root = beacon_block_header.root;

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

    /// Fetch the full beacon block for a header, then build the `SlotContext`.
    async fn fetch_slot_context(&self, beacon_block_header: &BlockHeader) -> Result<SlotContext> {
        let beacon_block = self
            .get_beacon_block_by_hash_with_retry(beacon_block_header.slot, beacon_block_header.root)
            .await?;
        Self::slot_context_from_block(beacon_block_header, &beacon_block)
    }

    /// Derive the full per-slot update from beacon/execution data and return it for commit.
    pub async fn derive_slot_update(
        &self,
        beacon_block_header: &BlockHeader,
    ) -> Result<ProcessedSlot> {
        let slot_ctx = self.fetch_slot_context(beacon_block_header).await?;
        self.derive_from_context(slot_ctx).await
    }

    /// Like `derive_slot_update`, but uses a pre-fetched beacon block instead of fetching it.
    pub async fn derive_slot_update_with_block(
        &self,
        beacon_block_header: &BlockHeader,
        beacon_block: &Block,
    ) -> Result<ProcessedSlot> {
        let slot_ctx = Self::slot_context_from_block(beacon_block_header, beacon_block)?;
        self.derive_from_context(slot_ctx).await
    }

    /// Parse the slot's blobs, prefetch the array positions of their created
    /// commitments that already exist in canonical state, and derive the next
    /// head.
    ///
    /// The prefetch is one batched query against the created index, mirroring how
    /// `recent_gsrs` is prefetched, so the state machine never has to query the
    /// database itself. It returns indices (not just a membership set) so the
    /// state machine can cross-check each hit against the array at the base root.
    async fn derive_slot(
        &self,
        base_head: CanonicalHead,
        recent_gsrs: Vec<(Hash, i64)>,
        slot: u32,
        block_number: u32,
        blob_payloads: &[(u32, Vec<u8>)],
    ) -> Result<DerivedSlot> {
        let parsed = self
            .state_machine
            .parse_blobs(blob_payloads, slot, block_number);
        let candidates: Vec<Hash> = parsed
            .iter()
            .flat_map(|(_, payload)| payload.live.iter().copied())
            .collect();
        let prior_indices = self.sync_db.created_indices(&candidates).await?;
        self.state_machine.derive_slot_head(
            base_head,
            recent_gsrs,
            slot,
            block_number,
            &parsed,
            &prior_indices,
        )
    }

    /// Shared derivation logic: given an already-resolved `SlotContext`, fetch execution data
    /// as needed, run the state machine, and return the processed slot.
    async fn derive_from_context(&self, slot_ctx: SlotContext) -> Result<ProcessedSlot> {
        let base_head = self.sync_db.current_head().await?;

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
            let derived = self
                .derive_slot(base_head, recent_gsrs, slot_ctx.slot, block_number, &[])
                .await?;
            return Ok(ProcessedSlot::Present {
                slot: slot_ctx.slot,
                block_root: slot_ctx.beacon_block_root,
                parent_root: slot_ctx.parent_root,
                block_number,
                derived,
            });
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
            let derived = self
                .derive_slot(base_head, recent_gsrs, slot_ctx.slot, block_number, &[])
                .await?;
            return Ok(ProcessedSlot::Present {
                slot: slot_ctx.slot,
                block_root: slot_ctx.beacon_block_root,
                parent_root: slot_ctx.parent_root,
                block_number,
                derived,
            });
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

        let derived = self
            .derive_slot(
                base_head,
                recent_gsrs,
                slot_ctx.slot,
                block_number,
                &blob_payloads,
            )
            .await?;

        Ok(ProcessedSlot::Present {
            slot: slot_ctx.slot,
            block_root: slot_ctx.beacon_block_root,
            parent_root: slot_ctx.parent_root,
            block_number,
            derived,
        })
    }

    /// Commit one processed slot to Postgres as the new canonical head, writing
    /// its created-index rows in the same transaction.
    pub async fn commit_slot(&self, processed: &ProcessedSlot) -> Result<()> {
        match processed {
            ProcessedSlot::Missing { slot, carried_head } => {
                self.sync_db
                    .commit_slot(
                        &CommittedSlotRecord::empty(*slot),
                        carried_head,
                        &HashMap::new(),
                    )
                    .await
            }
            ProcessedSlot::Present {
                slot,
                block_root,
                parent_root,
                block_number,
                derived,
            } => {
                let record = CommittedSlotRecord {
                    slot: *slot,
                    block_root: Some(*block_root),
                    parent_root: Some(*parent_root),
                    block_number: Some(*block_number),
                    current_gsr: derived.head.metadata.current_gsr,
                    is_empty: false,
                };
                self.sync_db
                    .commit_slot(&record, &derived.head, &derived.created_added)
                    .await
            }
        }
    }
}
