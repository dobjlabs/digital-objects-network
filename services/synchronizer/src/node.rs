use std::{collections::HashMap, sync::Arc, time::Duration};

use eth_clients::{
    beacon::{
        self,
        types::{Block, BlockAndRoot, BlockHeader, BlockId, ExecutionPayloadRef, KzgCommitment},
        BeaconClient,
    },
    common::{fetch_once_available, ClientError},
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
use tracing::{debug, info};

use crate::config::AppConfig;
use crate::don_state::{DonState, ExecutionBlock};
use crate::state_log_db::StateLogDb;
use crate::state_machine::{DerivedSlot, StateMachine, MAX_STATE_ROOT_AGE_BLOCKS};

/// Runtime integration layer that connects network inputs (beacon/execution),
/// pure state derivation (`StateMachine`), and the state log (`StateLogDb`).
pub struct Node {
    pub beacon_cli: BeaconClient,
    pub archiver_cli: BeaconClient,
    pub rpc_cli: RootProvider,
    pub config: AppConfig,
    pub state_machine: Arc<StateMachine>,
    pub state_log: Arc<StateLogDb>,
}

/// Everything fetched from the network to derive one present slot.
pub(crate) struct SlotContext {
    slot: u32,
    beacon_block_root: B256,
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
    execution_block: ExecutionBlock,
    kzg_blob_commitments: Vec<(B256, KzgCommitment)>,
    /// The payload's blob transactions sent to the target address, in block order.
    target_blob_txs: Vec<TargetBlobTx>,
}

struct TargetBlobTx {
    tx_hash: B256,
    versioned_hashes: Vec<B256>,
}

/// Outcome of processing one beacon slot, ready to be committed.
pub struct ProcessedSlot {
    pub slot: u32,
    /// `None` when the beacon chain produced no block for the slot.
    pub block_root: Option<B256>,
    /// `None` when no execution block became canonical with the slot.
    pub derived: Option<DerivedSlot>,
}

impl Node {
    /// Construct network clients and bind shared state/sync stores.
    pub async fn new(
        cfg: AppConfig,
        state_machine: Arc<StateMachine>,
        state_log: Arc<StateLogDb>,
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
            state_log,
        })
    }

    /// Check that `slot` can be the genesis slot: finalized, so it can never be reorged out, and
    /// with an execution block for the genesis state to be derived at. Waiting for finalization
    /// ends in a node error, so the sync loop retries it.
    pub(crate) async fn validate_genesis_slot(&self, slot: u32) -> Result<()> {
        fetch_once_available(
            self.config.availability_polling,
            "finalization of INIT_START_SLOT",
            || async {
                let finalized = self.beacon_cli.get_block_header(BlockId::Finalized).await?;
                if let Some(header) = &finalized {
                    info!(
                        INIT_START_SLOT = slot,
                        last_finalized_slot = header.slot,
                        "Waiting for INIT_START_SLOT to be finalized"
                    );
                }
                Ok(finalized.filter(|header| header.slot >= slot).map(|_| ()))
            },
        )
        .await?;
        let header = self
            .beacon_cli
            .get_block_header(BlockId::Slot(slot))
            .await?
            .ok_or_else(|| {
                anyhow!(
                    "INIT_START_SLOT {slot} has no beacon block; pick a finalized slot with an \
                     execution block"
                )
            })?;
        let block = self.get_beacon_block(header.root).await?;
        anyhow::ensure!(
            self.canonical_payload(&header, &block, None)
                .await?
                .is_some(),
            "INIT_START_SLOT {slot} has no execution block; pick a finalized slot with one"
        );
        Ok(())
    }

    pub(crate) async fn get_beacon_head_header(&self) -> Result<BlockHeader> {
        fetch_once_available(
            self.config.availability_polling,
            "beacon head header",
            || async { Ok(self.beacon_cli.get_block_header(BlockId::Head).await?) },
        )
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
        let what = format!("archived blobs of slot {slot}");
        let mut blobs = fetch_once_available(self.config.availability_polling, &what, || async {
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
        fetch_once_available(self.config.availability_polling, &what, || async {
            Ok(self
                .beacon_cli
                .get_block(BlockId::Hash(beacon_block_root))
                .await?)
        })
        .await
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
        let execution_block =
            fetch_once_available(self.config.availability_polling, &what, || async {
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
            execution_block: ExecutionBlock {
                number: execution_block.header.number.try_into()?,
                timestamp: execution_block.header.timestamp,
                hash: execution_block_hash,
            },
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
            payload,
            blob_payloads,
        })
    }

    /// Parse the slot's blobs, prefetch the array positions of their created
    /// commitments that already exist in committed state, and derive the next
    /// state.
    ///
    /// The prefetch is one batched query against the created index, mirroring how
    /// `recent_state_roots` is prefetched, so the state machine never has to query the
    /// database itself. It returns indices (not just a membership set) so the
    /// state machine can cross-check each hit against the array at the base root.
    async fn derive_slot(
        &self,
        base_state: Option<DonState>,
        recent_state_roots: Vec<(Hash, i64)>,
        slot: u32,
        block: ExecutionBlock,
        blob_payloads: &[(u32, Vec<u8>)],
    ) -> Result<DerivedSlot> {
        let parsed = self
            .state_machine
            .parse_blobs(blob_payloads, slot, block.number);
        let candidates: Vec<Hash> = parsed
            .iter()
            .flat_map(|(_, payload)| payload.created.iter().copied())
            .collect();
        let prior_indices = self.state_log.created_indices(&candidates).await?;
        self.state_machine.derive_slot_state(
            base_state,
            recent_state_roots,
            slot,
            block,
            &parsed,
            &prior_indices,
        )
    }

    /// Derive the slot from a fetched `SlotContext` against the committed state. Reads the
    /// sync database but makes no network calls.
    pub(crate) async fn derive_from_context(&self, slot_ctx: SlotContext) -> Result<ProcessedSlot> {
        let Some(payload) = slot_ctx.payload else {
            info!(
                slot = slot_ctx.slot,
                "No execution payload became canonical with this slot's block"
            );
            return Ok(ProcessedSlot {
                slot: slot_ctx.slot,
                block_root: Some(slot_ctx.beacon_block_root),
                derived: None,
            });
        };
        let base_state = self.state_log.head_state().await?;

        debug!(
            slot = slot_ctx.slot,
            execution_block_hash = ?payload.execution_block.hash,
            execution_block_number = payload.execution_block.number,
            "Resolved execution payload for slot"
        );
        info!(
            "Processing slot {} from {}",
            slot_ctx.slot,
            DateTime::<Utc>::from_timestamp_secs(payload.execution_block.timestamp as i64)
                .unwrap_or_default(),
        );
        match base_state {
            Some(base_state) => self.state_machine.log_state(base_state),
            None => info!(slot = slot_ctx.slot, "Deriving the genesis state"),
        }

        let min_block_number = base_state.map_or(0, |state| {
            state
                .metadata
                .block
                .number
                .saturating_sub(MAX_STATE_ROOT_AGE_BLOCKS as u32)
        });
        let recent_state_roots = self.state_log.recent_state_roots(min_block_number).await?;

        let derived = self
            .derive_slot(
                base_state,
                recent_state_roots,
                slot_ctx.slot,
                payload.execution_block,
                &slot_ctx.blob_payloads,
            )
            .await?;

        Ok(ProcessedSlot {
            slot: slot_ctx.slot,
            block_root: Some(slot_ctx.beacon_block_root),
            derived: Some(derived),
        })
    }

    /// Commit one processed slot to Postgres as the new head state, writing
    /// its created-index rows in the same transaction.
    pub async fn commit_slot(&self, processed: &ProcessedSlot) -> Result<()> {
        // Sync merkle trees to disk to guarantee that after this point the data is persisted on
        // disk even after a system crash.  The state is committed to Postgres next and its roots
        // must not outlive the merkle tree nodes in a crash.
        self.state_machine.sync()?;
        self.state_log
            .commit_slot(
                processed.slot,
                processed.block_root,
                processed.derived.as_ref(),
            )
            .await
    }
}
