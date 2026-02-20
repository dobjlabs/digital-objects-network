use std::{
    collections::HashMap,
    sync::RwLock,
    time::Duration,
};

use synchronizer::{
    clients::beacon::{
        self,
        types::{Blob, BlockHeader, BlockId},
        BeaconClient,
    },
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

use crate::config::AppConfig;
use crate::db::{Db, DerivedState, SyncProgress};

#[path = "state.rs"]
mod state;
use state::State;

pub struct Node {
    pub beacon_cli: BeaconClient,
    pub rpc_cli: RootProvider,
    pub config: AppConfig,
    db: Db,
    state: RwLock<State>,
}

struct SlotContext {
    slot: u32,
    beacon_block_root: B256,
    execution_block_hash: B256,
    execution_block_number: u32,
    execution_timestamp: u64,
    has_blob_commitments: bool,
}

impl Node {
    pub async fn new(cfg: AppConfig) -> Result<Self> {
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

        let db = Db::connect(&cfg.db_path).await?;
        db.init().await?;
        let DerivedState {
            transactions,
            nullifiers,
        } = db.load_state().await?;
        let state = State {
            transactions,
            nullifiers,
        };

        Ok(Self {
            beacon_cli,
            rpc_cli,
            config: cfg,
            db,
            state: RwLock::new(state),
        })
    }

    pub async fn last_processed_slot(&self) -> Result<Option<u32>> {
        self.db.last_processed_slot().await
    }

    pub async fn last_progress(&self) -> Result<Option<SyncProgress>> {
        self.db.last_progress().await
    }

    pub async fn mark_slot_processed(&self, slot: u32, block_number: Option<u32>) -> Result<()> {
        self.db.mark_slot_processed(slot, block_number).await
    }

    async fn get_blobs(&self, slot: u32, versioned_hashes: &[B256]) -> Result<HashMap<B256, Blob>> {
        let blobs = self.beacon_cli.get_blobs(slot.into()).await?;
        debug!("got {} blobs from beacon_cli", blobs.len());
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
                return Err(anyhow!("Blob {} not found in beacon_cli response", vh));
            }
        }

        Ok(blobs)
    }

    fn log_slot_start(ctx: &SlotContext) {
        info!(
            "processing slot {} from {}",
            ctx.slot,
            DateTime::<Utc>::from_timestamp_secs(ctx.execution_timestamp as i64)
                .unwrap_or_default(),
        );
    }

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
                debug!("slot {} has empty block", slot);
                return Ok(None);
            }
        };

        let execution_payload = match beacon_block.execution_payload.as_ref() {
            Some(payload) => payload,
            None => {
                debug!("slot {} has no execution payload", slot);
                return Ok(None);
            }
        };

        let has_blob_commitments = self.has_blob_commitments(&beacon_block);

        Ok(Some(SlotContext {
            slot,
            beacon_block_root,
            execution_block_hash: execution_payload.block_hash,
            execution_block_number: execution_payload.block_number,
            execution_timestamp: execution_payload.timestamp,
            has_blob_commitments,
        }))
    }

    fn has_blob_commitments(
        &self,
        beacon_block: &synchronizer::clients::beacon::types::Block,
    ) -> bool {
        match &beacon_block.blob_kzg_commitments {
            Some(commitments) => !commitments.is_empty(),
            None => false,
        }
    }

    pub async fn process_beacon_block_header(
        &self,
        beacon_block_header: &BlockHeader,
    ) -> Result<Option<u32>> {
        let Some(slot_ctx) = self.build_slot_context(beacon_block_header).await? else {
            return Ok(None);
        };

        debug!(
            "slot {} has execution block {} at height {}",
            slot_ctx.slot, slot_ctx.execution_block_hash, slot_ctx.execution_block_number
        );
        Self::log_slot_start(&slot_ctx);
        self.log_current_state()?;
        if !slot_ctx.has_blob_commitments {
            debug!("slot {} has no blobs", slot_ctx.slot);
            return Ok(Some(slot_ctx.execution_block_number));
        }

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
                // Target Digital Object blob txs: blob-carrying txs sent to configured recipient.
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
            return Ok(Some(slot_ctx.execution_block_number));
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
                .map(|blob_versioned_hash| &blobs[blob_versioned_hash])
                .collect();
            trace!(?hash, ?from, ?to);

            for blob in tx_blobs.iter() {
                self.process_do_blob(blob, slot_ctx.slot, Some(slot_ctx.execution_block_number))
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to process do_blob at slot {}, blob_index {}",
                            slot_ctx.slot, blob.index
                        )
                    })?;
                info!(
                    "Valid do_blob at slot {}, blob_index {}!",
                    slot_ctx.slot, blob.index
                );
            }
        }
        Ok(Some(slot_ctx.execution_block_number))
    }
}
