use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
    sync::RwLock,
    time::Duration,
};

use synchronizer::{
    bytes_from_simple_blob,
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
    primitives::{Address, B256},
    providers as alloy_provider,
    transports::http::reqwest,
};
use alloy_network::Ethereum;
use alloy_provider::{Provider, RootProvider};
use anyhow::{anyhow, Context, Result};
use backoff::ExponentialBackoffBuilder;
use chrono::{DateTime, Utc};
use tracing::{debug, info, trace};

use crate::db::{Db, DerivedState};

#[derive(Debug)]
pub struct State {
    transactions: HashSet<String>,
    nullifiers: HashSet<String>,
}

pub struct Node {
    pub beacon_cli: BeaconClient,
    pub rpc_cli: RootProvider,
    db: Db,
    // Mutable state.
    state: RwLock<State>,
}

// This node code is adapted from https://github.com/0xPARC/digital-objects-e2e-poc/blob/main/synchronizer/src/main.rs
impl Node {
    pub async fn new(database_url: &str) -> Result<Self> {
        let http_cli = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()?;
        let rpc_url: String = dotenvy::var("RPC_URL")?;
        let beacon_url: String = dotenvy::var("BEACON_URL")?;

        let exp_backoff = Some(ExponentialBackoffBuilder::default().build());
        let beacon_cli_cfg = beacon::Config {
            base_url: beacon_url,
            exp_backoff,
        };
        let beacon_cli = BeaconClient::try_with_client(http_cli, beacon_cli_cfg)?;
        let rpc_cli = RootProvider::<Ethereum>::new_http(rpc_url.parse()?);

        let db = Db::connect(database_url).await?;
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
            db,
            state: RwLock::new(state),
        })
    }

    pub async fn last_processed_slot(&self) -> Result<Option<u32>> {
        self.db.last_processed_slot().await
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

    pub async fn process_beacon_block_header(
        &self,
        beacon_block_header: &BlockHeader,
    ) -> Result<Option<u32>> {
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
        let execution_payload = match beacon_block.execution_payload {
            Some(payload) => payload,
            None => {
                debug!("slot {} has no execution payload", slot);
                return Ok(None);
            }
        };
        debug!(
            "slot {} has execution block {} at height {}",
            slot, execution_payload.block_hash, execution_payload.block_number
        );

        info!(
            "processing slot {} from {}",
            slot,
            DateTime::<Utc>::from_timestamp_secs(execution_payload.timestamp as i64)
                .unwrap_or_default(),
        );
        info!(
            "current state: transactions={:?}, nullifiers={:?}, ",
            self.state.read().expect("lock").transactions,
            self.state.read().expect("lock").nullifiers,
        );

        let has_kzg_blob_commitments = match beacon_block.blob_kzg_commitments {
            Some(commitments) => !commitments.is_empty(),
            None => false,
        };
        if !has_kzg_blob_commitments {
            debug!("slot {} has no blobs", slot);
            return Ok(Some(execution_payload.block_number));
        }

        let execution_block_hash = execution_payload.block_hash;

        let execution_block_id = alloy_eips::eip1898::BlockId::Hash(execution_block_hash.into());
        let execution_block = self
            .rpc_cli
            .get_block(execution_block_id)
            .full()
            .await?
            .with_context(|| format!("Execution block {execution_block_hash} not found"))?;

        let to_addr: Address = Address::from_str(&dotenvy::var("TO_ADDRESS")?)?;
        let indexed_do_blob_txs: Vec<_> = match execution_block.transactions.as_transactions() {
            Some(txs) => txs
                .iter()
                .enumerate()
                .filter(|(_index, tx)| {
                    tx.inner.blob_versioned_hashes().is_some()
                        && tx.as_recovered().to() == Some(to_addr)
                })
                .collect(),
            None => {
                return Err(anyhow!(
                    "Consensus block {beacon_block_root} has blobs but the execution block doesn't have txs"
                ));
            }
        };

        if indexed_do_blob_txs.is_empty() {
            return Ok(Some(execution_payload.block_number));
        }

        let txs_blobs_vhs: Vec<B256> = indexed_do_blob_txs
            .iter()
            .flat_map(|(_, tx)| {
                tx.as_recovered()
                    .blob_versioned_hashes()
                    .expect("tx has blobs")
            })
            .cloned()
            .collect();
        let blobs = self.get_blobs(slot, &txs_blobs_vhs).await?;

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
                match self
                    .process_do_blob(blob, slot, Some(execution_payload.block_number))
                    .await
                {
                    Ok(_) => {
                        info!("Valid do_blob at slot {}, blob_index {}!", slot, blob.index);
                    }
                    Err(e) => {
                        info!("Invalid do_blob: {:?}", e);
                        continue;
                    }
                };
            }
        }
        Ok(Some(execution_payload.block_number))
    }
}

impl Node {
    // This processes the digital object blob and updates in-memory and persisted state.
    async fn process_do_blob(
        &self,
        blob: &Blob,
        slot: u32,
        block_number: Option<u32>,
    ) -> Result<()> {
        let bytes =
            bytes_from_simple_blob(blob.blob.inner()).context("Invalid byte encoding in blob")?;
        let commit_proof_hash = hex::encode(bytes);
        info!("Processing commitment {}", commit_proof_hash);

        let inserted = {
            let mut state = self.state.write().expect("lock");
            state.transactions.insert(commit_proof_hash.clone())
        };

        if inserted {
            self.db
                .persist_transaction(&commit_proof_hash, slot, block_number)
                .await?;
        }

        info!(
            "state update: transactions={:?}, nullifiers={:?}, ",
            self.state.read().expect("lock").transactions,
            self.state.read().expect("lock").nullifiers,
        );
        Ok(())
    }
}
