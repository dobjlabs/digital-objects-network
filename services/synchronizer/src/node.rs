use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

use eth_clients::{
    beacon::{
        self,
        types::{Block, BlockAndRoot, BlockHeader, BlockId, ExecutionPayloadRef, KzgCommitment},
        BeaconClient,
    },
    common::ClientError,
    execution,
};

use alloy::{
    consensus::Transaction,
    eips::eip4844::{kzg_to_versioned_hash, HeapBlob},
    primitives::B256,
    providers as alloy_provider,
    rpc::types::BlockTransactionsKind,
};
use alloy_provider::{Provider, RootProvider};
use anyhow::{anyhow, Context, Result};
use backoff::ExponentialBackoffBuilder;
use chrono::{DateTime, Utc};
use pod2::middleware::Hash;
use pod2utils::b256_to_hash;
use tracing::{debug, info, warn};

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

/// Everything fetched from the network to derive one present slot.
pub(crate) struct SlotContext {
    slot: u32,
    beacon_block_root: B256,
    parent_root: B256,
    /// `None` when no payload becomes canonical with this block, which from Gloas on happens
    /// when its bid does not build on the parent's payload (withheld or not revealed in time).
    payload: Option<CanonicalPayload>,
    /// Decoded target blobs of `payload`, each with its index among the committing block's
    /// blobs. Empty without a payload.
    blob_payloads: Vec<(u32, Vec<u8>)>,
}

/// An execution payload that becomes canonical with a slot's block, together with the beacon
/// block whose bid committed to its blobs. From Gloas on that is the parent of the slot's block.
struct CanonicalPayload {
    committing_block_root: B256,
    execution_block_hash: B256,
    execution_block_number: u32,
    execution_block_timestamp: u64,
    kzg_blob_commitments: Vec<(B256, KzgCommitment)>,
    /// The payload's blob transactions sent to the target address, in block order.
    target_blob_txs: Vec<TargetBlobTx>,
}

struct TargetBlobTx {
    tx_hash: B256,
    versioned_hashes: Vec<B256>,
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

        let exp_backoff = ExponentialBackoffBuilder::default().build();
        let beacon_cli_cfg = beacon::Config {
            base_url: cfg.beacon_url.clone(),
            exp_backoff: exp_backoff.clone(),
        };
        let beacon_cli = BeaconClient::try_with_client(http_cli.clone(), beacon_cli_cfg)?;
        let archiver_cli_cfg = beacon::Config {
            base_url: cfg.archiver_url.clone(),
            exp_backoff: exp_backoff.clone(),
        };
        let archiver_cli = BeaconClient::try_with_client(http_cli.clone(), archiver_cli_cfg)?;
        let rpc_cli = execution::connect(http_cli, cfg.rpc_url.parse()?, exp_backoff);

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

    /// Ask `fetch` again while it finds nothing, as a node lagging behind the chain may not
    /// serve `what` yet. Failed requests are retried inside the clients, so an error ends the
    /// wait.
    async fn fetch_once_available<T, Fut>(
        &self,
        what: &str,
        mut fetch: impl FnMut() -> Fut,
    ) -> Result<T>
    where
        Fut: Future<Output = Result<Option<T>>>,
    {
        for retry in 0..=self.config.rpc_retries {
            if let Some(value) = fetch().await? {
                return Ok(value);
            }
            if retry < self.config.rpc_retries {
                warn!(
                    what,
                    retry = retry + 1,
                    max_retries = self.config.rpc_retries,
                    "Not available yet; asking again"
                );
                tokio::time::sleep(self.config.rpc_retry_delay).await;
            }
        }
        Err(anyhow!(
            "{what} still not available after {} retries",
            self.config.rpc_retries
        ))
    }

    pub(crate) async fn get_beacon_head_header(&self) -> Result<BlockHeader> {
        self.beacon_cli
            .get_block_header(BlockId::Head)
            .await?
            .ok_or_else(|| anyhow!("Beacon head header not found"))
    }

    /// Fetch beacon blob sidecars for a slot and retain only requested versioned hashes.
    async fn get_blobs(
        &self,
        slot: u32,
        beacon_block_root: &B256,
        block_kzg_blob_commitments: &[(B256, KzgCommitment)],
        versioned_hashes: &[B256],
    ) -> Result<HashMap<B256, HeapBlob>> {
        let what = format!("archived blobs of slot {slot}");
        let mut blobs = self
            .fetch_once_available(&what, || async {
                match self
                    .archiver_cli
                    .get_blobs((*beacon_block_root).into(), versioned_hashes)
                    .await
                {
                    Ok(blobs) => Ok(Some(blobs)),
                    Err(ClientError::NotFound(_)) => Ok(None),
                    Err(err) => Err(err.into()),
                }
            })
            .await?;

        if blobs.len() != versioned_hashes.len() {
            return Err(anyhow!(
                "Fetched {} blobs, but got {}",
                versioned_hashes.len(),
                blobs.len()
            ));
        }
        debug!(
            slot,
            blob_count = blobs.len(),
            "Fetched blobs from archiver"
        );

        blobs.reverse();
        let mut blob_map = HashMap::new();
        for (vh, _) in block_kzg_blob_commitments {
            if versioned_hashes.contains(vh) {
                blob_map.insert(*vh, blobs.pop().expect("not empty"));
            }
        }

        Ok(blob_map)
    }

    pub(crate) async fn get_beacon_block(&self, beacon_block_root: B256) -> Result<Block> {
        let what = format!("beacon block {beacon_block_root}");
        self.fetch_once_available(&what, || async {
            Ok(self
                .beacon_cli
                .get_block(BlockId::Hash(beacon_block_root))
                .await?)
        })
        .await
    }

    pub(crate) async fn load_committed_slot_record(
        &self,
        slot: u32,
    ) -> Result<CommittedSlotRecord> {
        let Some(header) = self
            .beacon_cli
            .get_block_header(BlockId::Slot(slot))
            .await?
        else {
            return Ok(CommittedSlotRecord::empty(slot));
        };

        let block = self.get_beacon_block(header.root).await?;
        let payload = self.canonical_payload(&header, &block, None).await?;

        Ok(CommittedSlotRecord {
            slot,
            block_root: Some(header.root),
            parent_root: Some(block.parent_root),
            block_number: payload.map(|payload| payload.execution_block_number),
            current_state_root: None,
            is_empty: false,
        })
    }

    /// Resolve the execution payload that becomes canonical with `beacon_block`: its own
    /// payload before Gloas, its parent's from Gloas on when its bid builds on it. The parent
    /// block is fetched unless `prev_block` is the parent.
    ///
    /// A payload that should be canonical but cannot be fetched is an error rather than an
    /// empty slot. This forces the sync loop to retry instead of silently advancing past a real
    /// payload when the beacon or execution provider is temporarily inconsistent.
    async fn canonical_payload(
        &self,
        beacon_block_header: &BlockHeader,
        beacon_block: &Block,
        prev_block: Option<&BlockAndRoot>,
    ) -> Result<Option<CanonicalPayload>> {
        let slot = beacon_block_header.slot;
        let fetched_parent_block;
        let (committing_block_root, committing_block) = match beacon_block.execution_payload {
            ExecutionPayloadRef::Embedded { .. } => (beacon_block_header.root, beacon_block),
            ExecutionPayloadRef::Bid { .. } => {
                let parent_block = match prev_block {
                    Some(prev) if prev.root == beacon_block.parent_root => &prev.block,
                    _ => {
                        fetched_parent_block =
                            self.get_beacon_block(beacon_block.parent_root).await?;
                        &fetched_parent_block
                    }
                };
                if !beacon_block.builds_on_payload_of(parent_block) {
                    debug!(slot, "Block does not build on its parent's payload");
                    return Ok(None);
                }
                (beacon_block.parent_root, parent_block)
            }
        };

        let kzg_blob_commitments: Vec<(B256, KzgCommitment)> = committing_block
            .blob_kzg_commitments
            .iter()
            .map(|c| (kzg_to_versioned_hash(c.as_ref()), *c))
            .collect();

        // The beacon block names its execution payload only by hash, so the number and
        // timestamp are read from the execution layer. Transactions are only needed to find
        // the target blob transactions.
        let execution_block_hash = committing_block.execution_payload.block_hash();
        let transactions_kind = if kzg_blob_commitments.is_empty() {
            BlockTransactionsKind::Hashes
        } else {
            BlockTransactionsKind::Full
        };
        let what = format!("execution block {execution_block_hash} of slot {slot}");
        let execution_block = self
            .fetch_once_available(&what, || async {
                Ok(self
                    .rpc_cli
                    .get_block(execution_block_hash.into())
                    .kind(transactions_kind)
                    .await?)
            })
            .await?;

        let target_blob_txs = match execution_block.transactions.as_transactions() {
            Some(txs) => txs
                .iter()
                .filter(|tx| tx.as_recovered().to() == Some(self.config.to_address))
                .filter_map(|tx| {
                    let versioned_hashes = tx.inner.blob_versioned_hashes()?;
                    Some(TargetBlobTx {
                        tx_hash: *tx.inner.hash(),
                        versioned_hashes: versioned_hashes.to_vec(),
                    })
                })
                .collect(),
            None if kzg_blob_commitments.is_empty() => Vec::new(),
            None => {
                return Err(anyhow!(
                    "Consensus block {committing_block_root} has blobs but the execution block doesn't have txs"
                ));
            }
        };

        Ok(Some(CanonicalPayload {
            committing_block_root,
            execution_block_hash,
            execution_block_number: execution_block.header.number.try_into()?,
            execution_block_timestamp: execution_block.header.timestamp,
            kzg_blob_commitments,
            target_blob_txs,
        }))
    }

    /// Fetch and decode the blobs of the payload's target blob transactions, each with its
    /// index among the committing block's blobs.
    async fn fetch_target_blobs(
        &self,
        slot: u32,
        payload: &CanonicalPayload,
    ) -> Result<Vec<(u32, Vec<u8>)>> {
        if payload.target_blob_txs.is_empty() {
            debug!(
                slot,
                blob_count = payload.kzg_blob_commitments.len(),
                to_address = ?self.config.to_address,
                "No target blob transactions in execution block"
            );
            return Ok(Vec::new());
        }

        let blob_versioned_hashes: Vec<B256> = payload
            .target_blob_txs
            .iter()
            .flat_map(|tx| tx.versioned_hashes.iter().copied())
            .collect();
        let blobs = self
            .get_blobs(
                slot,
                &payload.committing_block_root,
                &payload.kzg_blob_commitments,
                &blob_versioned_hashes,
            )
            .await?;

        let mut blob_payloads = Vec::new();
        for tx in &payload.target_blob_txs {
            for vh in &tx.versioned_hashes {
                let blob_index = payload
                    .kzg_blob_commitments
                    .iter()
                    .position(|(vh0, _)| vh0 == vh)
                    .expect("vh exists");
                let bytes =
                    payload::blob::decode_simple_blob(blobs[vh].inner()).with_context(|| {
                        format!(
                            "Invalid byte encoding in blob at slot {slot}, blob_index {blob_index}"
                        )
                    })?;
                blob_payloads.push((blob_index as u32, bytes));
                info!(
                    slot,
                    blob_index = blob_index,
                    tx_hash = ?tx.tx_hash,
                    "Decoded target blob"
                );
            }
        }
        Ok(blob_payloads)
    }

    /// Make every network call needed to derive the slot of `beacon_block`. `prev_block`
    /// spares fetching the parent again when the caller already holds it.
    pub(crate) async fn fetch_slot_context(
        &self,
        beacon_block_header: &BlockHeader,
        beacon_block: &Block,
        prev_block: Option<&BlockAndRoot>,
    ) -> Result<SlotContext> {
        let slot = beacon_block_header.slot;
        let payload = self
            .canonical_payload(beacon_block_header, beacon_block, prev_block)
            .await?;
        let blob_payloads = match &payload {
            Some(payload) => self.fetch_target_blobs(slot, payload).await?,
            None => Vec::new(),
        };
        Ok(SlotContext {
            slot,
            beacon_block_root: beacon_block_header.root,
            parent_root: beacon_block.parent_root,
            payload,
            blob_payloads,
        })
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
            .flat_map(|(_, payload)| payload.created.iter().copied())
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

    /// Derive the slot from a fetched `SlotContext` against the committed state. Reads the
    /// sync database but makes no network calls.
    pub(crate) async fn derive_from_context(&self, slot_ctx: SlotContext) -> Result<ProcessedSlot> {
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
        let derived = self
            .derive_slot(
                base_head,
                recent_state_roots,
                slot_ctx.slot,
                block_meta,
                &slot_ctx.blob_payloads,
            )
            .await?;

        Ok(ProcessedSlot::Present {
            slot: slot_ctx.slot,
            block_root: slot_ctx.beacon_block_root,
            parent_root: slot_ctx.parent_root,
            block_number: payload.execution_block_number,
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
