use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

use eth_clients::beacon::{
    self,
    types::{Block, BlockHeader, BlockId, ExecutionPayloadRef, KzgCommitment, Spec},
    BeaconClient,
};

use alloy::{
    consensus::Transaction,
    eips::{
        self as alloy_eips,
        eip4844::{kzg_to_versioned_hash, HeapBlob},
    },
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
use pod2utils::b256_to_hash;
use tracing::{debug, info, trace, warn};

use crate::config::AppConfig;
use crate::head::{BlockMetadata, StateHead};
use crate::state_machine::{DerivedSlot, StateMachine, MAX_STATE_ROOT_AGE_BLOCKS};
use crate::sync_db::{CommittedSlotRecord, SyncDb};

/// Runtime integration layer that connects network inputs (beacon/execution),
/// pure state derivation (`StateMachine`), and sync metadata (`SyncDb`).
pub struct Node {
    pub beacon_cli: BeaconClient,
    pub archiver_cli: BeaconClient,
    pub rpc_cli: RootProvider,
    pub config: AppConfig,
    pub state_machine: Arc<StateMachine>,
    pub sync_db: Arc<SyncDb>,
}

struct SlotContext {
    slot: u32,
    beacon_block_root: B256,
    parent_root: B256,
    /// `None` when no payload becomes canonical with this block, which from Gloas on happens
    /// when its bid does not build on the parent's payload (withheld or not revealed in time).
    payload: Option<CanonicalPayload>,
}

/// An execution payload that becomes canonical with a slot's block, together with the beacon
/// block whose bid committed to its blobs. From Gloas on that is the parent of the slot's block.
struct CanonicalPayload {
    committing_block_root: B256,
    execution_block_hash: B256,
    execution_block_number: u32,
    execution_block_timestamp: u64,
    kzg_blob_commitments: Vec<(B256, KzgCommitment)>,
}

pub(crate) struct ExecutionHeader {
    number: u32,
    timestamp: u64,
}

/// Outcome of processing one beacon slot, ready to be committed.
pub enum ProcessedSlot {
    /// Beacon produced no block for the slot. With no execution block there is
    /// nothing to derive against, so the previous state head is carried
    /// forward unchanged, committed under the new slot number to keep the
    /// slot history contiguous.
    Missing { slot: u32, carried_head: StateHead },
    /// Beacon produced a block, but no execution payload became canonical with it. The state
    /// head is carried forward as for `Missing`, while the block root is kept for reorg checks.
    WithoutPayload {
        slot: u32,
        block_root: B256,
        parent_root: B256,
        carried_head: StateHead,
    },
    /// Beacon produced a block and the state machine derived the slot against
    /// it (deriving a fresh state root even when the block carries no usable blobs).
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
            exp_backoff: exp_backoff.clone(),
        };
        let beacon_cli = BeaconClient::try_with_client(http_cli.clone(), beacon_cli_cfg)?;
        let archiver_cli_cfg = beacon::Config {
            base_url: cfg.archiver_url.clone(),
            exp_backoff,
        };
        let archiver_cli = BeaconClient::try_with_client(http_cli, archiver_cli_cfg)?;
        let rpc_cli = RootProvider::<Ethereum>::new_http(cfg.rpc_url.parse()?);

        Ok(Self {
            beacon_cli,
            archiver_cli,
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

    pub async fn current_head(&self) -> Result<StateHead> {
        self.sync_db.current_head().await
    }

    /// Rewind to `keep_slot` by deleting later slot rows; the created
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
        beacon_block_root: &B256,
        block_kzg_blob_commitments: &[(B256, KzgCommitment)],
        versioned_hashes: &[B256],
    ) -> Result<HashMap<B256, HeapBlob>> {
        let blobs = self
            .retry_rpc("beacon blob sidecars", format!("slot {slot}"), || async {
                let mut blobs = self
                    .archiver_cli
                    .get_blobs((*beacon_block_root).into(), versioned_hashes)
                    .await?;
                if blobs.len() != versioned_hashes.len() {
                    return Err(anyhow!(
                        "Fetched {} blobs, but got {}",
                        versioned_hashes.len(),
                        blobs.len()
                    ));
                }
                blobs.reverse();
                let mut blob_map = HashMap::new();
                for (vh, _) in block_kzg_blob_commitments {
                    if versioned_hashes.contains(vh) {
                        blob_map.insert(*vh, blobs.pop().expect("not empty"));
                    }
                }

                Ok(blob_map)
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
        let payload = self.canonical_payload(&header, &block).await?;

        Ok(CommittedSlotRecord {
            slot,
            block_root: Some(header.root),
            parent_root: Some(block.parent_root),
            block_number: payload.map(|payload| payload.execution_block_number),
            current_state_root: None,
            is_empty: false,
        })
    }

    /// The beacon block names its execution payload only by hash, so the number and timestamp
    /// are read from the execution layer.
    pub(crate) async fn get_execution_header_with_retry(
        &self,
        slot: u32,
        execution_block_hash: B256,
    ) -> Result<ExecutionHeader> {
        self.retry_rpc(
            "execution block header",
            format!("slot {slot}, block_hash {execution_block_hash}"),
            || async {
                let block = self
                    .rpc_cli
                    .get_block_by_hash(execution_block_hash)
                    .await?
                    .ok_or_else(|| anyhow!("Execution block {execution_block_hash} not found"))?;
                Ok(ExecutionHeader {
                    number: block.header.number.try_into()?,
                    timestamp: block.header.timestamp,
                })
            },
        )
        .await
    }

    /// Resolve the execution payload that becomes canonical with `beacon_block`: its own
    /// payload before Gloas, its parent's from Gloas on when its bid builds on it.
    ///
    /// A payload that should be canonical but cannot be fetched is an error rather than an
    /// empty slot. This forces the sync loop to retry instead of silently advancing past a real
    /// payload when the beacon or execution provider is temporarily inconsistent.
    async fn canonical_payload(
        &self,
        beacon_block_header: &BlockHeader,
        beacon_block: &Block,
    ) -> Result<Option<CanonicalPayload>> {
        let slot = beacon_block_header.slot;
        let parent_block;
        let (committing_block_root, committing_block) = match beacon_block.execution_payload {
            ExecutionPayloadRef::Embedded { .. } => (beacon_block_header.root, beacon_block),
            ExecutionPayloadRef::Bid { .. } => {
                parent_block = self
                    .get_beacon_block_by_hash_with_retry(slot, beacon_block.parent_root)
                    .await?;
                if !beacon_block.builds_on_payload_of(&parent_block) {
                    debug!(slot, "Block does not build on its parent's payload");
                    return Ok(None);
                }
                (beacon_block.parent_root, &parent_block)
            }
        };

        let execution_block_hash = committing_block.execution_payload.block_hash();
        let execution_header = self
            .get_execution_header_with_retry(slot, execution_block_hash)
            .await?;

        let kzg_blob_commitments = committing_block
            .blob_kzg_commitments
            .iter()
            .map(|c| (kzg_to_versioned_hash(c.as_ref()), *c))
            .collect();

        Ok(Some(CanonicalPayload {
            committing_block_root,
            execution_block_hash,
            execution_block_number: execution_header.number,
            execution_block_timestamp: execution_header.timestamp,
            kzg_blob_commitments,
        }))
    }

    /// Build a `SlotContext` from a beacon header and its full block.
    async fn slot_context_from_block(
        &self,
        beacon_block_header: &BlockHeader,
        beacon_block: &Block,
    ) -> Result<SlotContext> {
        Ok(SlotContext {
            slot: beacon_block_header.slot,
            beacon_block_root: beacon_block_header.root,
            parent_root: beacon_block.parent_root,
            payload: self
                .canonical_payload(beacon_block_header, beacon_block)
                .await?,
        })
    }

    /// Fetch the full beacon block for a header, then build the `SlotContext`.
    async fn fetch_slot_context(&self, beacon_block_header: &BlockHeader) -> Result<SlotContext> {
        let beacon_block = self
            .get_beacon_block_by_hash_with_retry(beacon_block_header.slot, beacon_block_header.root)
            .await?;
        self.slot_context_from_block(beacon_block_header, &beacon_block)
            .await
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
        let slot_ctx = self
            .slot_context_from_block(beacon_block_header, beacon_block)
            .await?;
        self.derive_from_context(slot_ctx).await
    }

    /// Parse the slot's blobs, prefetch the array positions of their created
    /// commitments that already exist in committed state, and derive the next
    /// head.
    ///
    /// The prefetch is one batched query against the created index, mirroring how
    /// `recent_state_roots` is prefetched, so the state machine never has to query the
    /// database itself. It returns indices (not just a membership set) so the
    /// state machine can cross-check each hit against the array at the base root.
    async fn derive_slot(
        &self,
        base_head: StateHead,
        recent_state_roots: Vec<(Hash, i64)>,
        slot: u32,
        block_meta: BlockMetadata,
        blob_payloads: &[(u32, Vec<u8>)],
    ) -> Result<DerivedSlot> {
        let parsed = self
            .state_machine
            .parse_blobs(blob_payloads, slot, block_meta.number);
        let candidates: Vec<Hash> = parsed
            .iter()
            .flat_map(|(_, payload)| payload.live.iter().copied())
            .collect();
        let prior_indices = self.sync_db.created_indices(&candidates).await?;
        self.state_machine.derive_slot_head(
            base_head,
            recent_state_roots,
            slot,
            block_meta,
            &parsed,
            &prior_indices,
        )
    }

    /// Shared derivation logic: given an already-resolved `SlotContext`, fetch execution data
    /// as needed, run the state machine, and return the processed slot.
    async fn derive_from_context(&self, slot_ctx: SlotContext) -> Result<ProcessedSlot> {
        let base_head = self.sync_db.current_head().await?;

        let Some(payload) = slot_ctx.payload else {
            info!(
                slot = slot_ctx.slot,
                "No execution payload became canonical with this slot's block"
            );
            return Ok(ProcessedSlot::WithoutPayload {
                slot: slot_ctx.slot,
                block_root: slot_ctx.beacon_block_root,
                parent_root: slot_ctx.parent_root,
                carried_head: base_head,
            });
        };

        debug!(
            slot = slot_ctx.slot,
            execution_block_hash = ?payload.execution_block_hash,
            execution_block_number = payload.execution_block_number,
            "Resolved execution payload for slot"
        );
        info!(
            "Processing slot {} from {}",
            slot_ctx.slot,
            DateTime::<Utc>::from_timestamp_secs(payload.execution_block_timestamp as i64)
                .unwrap_or_default(),
        );
        self.state_machine.log_current_state(base_head);

        let block_number = payload.execution_block_number;
        let min_block_number = base_head.metadata.current_block.map(|block_meta| {
            block_meta
                .number
                .saturating_sub(MAX_STATE_ROOT_AGE_BLOCKS as u32)
        });
        let recent_state_roots = self.sync_db.recent_state_roots(min_block_number).await?;

        let block_meta = BlockMetadata {
            number: payload.execution_block_number,
            timestamp: payload.execution_block_timestamp,
            hash: b256_to_hash(payload.execution_block_hash),
        };
        if payload.kzg_blob_commitments.is_empty() {
            debug!(slot = slot_ctx.slot, "Slot has no blob commitments");
            let derived = self
                .derive_slot(
                    base_head,
                    recent_state_roots,
                    slot_ctx.slot,
                    block_meta,
                    &[],
                )
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
                    slot_ctx.slot, payload.execution_block_hash
                ),
                || async {
                    let execution_block_id =
                        alloy_eips::eip1898::BlockId::Hash(payload.execution_block_hash.into());
                    self.rpc_cli
                        .get_block(execution_block_id)
                        .full()
                        .await?
                        .ok_or_else(|| {
                            anyhow!("Execution block {} not found", payload.execution_block_hash)
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
                    payload.committing_block_root
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
                .derive_slot(
                    base_head,
                    recent_state_roots,
                    slot_ctx.slot,
                    block_meta,
                    &[],
                )
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
            .get_blobs(
                slot_ctx.slot,
                &payload.committing_block_root,
                &payload.kzg_blob_commitments,
                &blob_versioned_hashes,
            )
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
                .map(|vh| (*vh, &blobs[vh]))
                .collect();
            trace!(?hash, ?from, ?to);

            for (vh, blob) in tx_blobs.iter() {
                let blob_index = payload
                    .kzg_blob_commitments
                    .iter()
                    .position(|(vh0, _)| vh0 == vh)
                    .expect("vh exists");
                let bytes = payload::blob::decode_simple_blob(blob.inner()).with_context(|| {
                    format!(
                        "Invalid byte encoding in blob at slot {}, blob_index {}",
                        slot_ctx.slot, blob_index
                    )
                })?;
                blob_payloads.push((blob_index as u32, bytes));
                info!(
                    slot = slot_ctx.slot,
                    blob_index = blob_index,
                    tx_hash = ?hash,
                    "Decoded target blob"
                );
            }
        }

        let derived = self
            .derive_slot(
                base_head,
                recent_state_roots,
                slot_ctx.slot,
                block_meta,
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

    /// Commit one processed slot to Postgres as the new state head, writing
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
            ProcessedSlot::WithoutPayload {
                slot,
                block_root,
                parent_root,
                carried_head,
            } => {
                let record = CommittedSlotRecord {
                    slot: *slot,
                    block_root: Some(*block_root),
                    parent_root: Some(*parent_root),
                    block_number: None,
                    current_state_root: None,
                    is_empty: false,
                };
                self.sync_db
                    .commit_slot(&record, carried_head, &HashMap::new())
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
                    current_state_root: derived.head.metadata.current_state_root,
                    is_empty: false,
                };
                self.sync_db
                    .commit_slot(&record, &derived.head, &derived.created_added)
                    .await
            }
        }
    }
}
