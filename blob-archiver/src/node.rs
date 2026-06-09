use std::time::Duration;

use alloy::eips::eip4844::HeapBlob;
use eth_clients::beacon::{
    self,
    types::{BlockHeader, BlockId, KzgCommitment},
    BeaconClient,
};
use itertools::zip_eq;
use std::os::unix;
use std::{
    fs::{self, create_dir_all, read_dir, rename, File},
    io,
    io::{Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use alloy::network::Ethereum;
use alloy::providers::{Provider, RootProvider};
use alloy::{
    consensus::Transaction,
    eips::{self as alloy_eips, eip4844::kzg_to_versioned_hash},
    primitives::{Bytes, B256},
};
use anyhow::{anyhow, Context, Result};
use backoff::ExponentialBackoffBuilder;
use chrono::{DateTime, Utc};
use tracing::{debug, info};

use crate::config::Config;

fn blob_file_name(index: usize, vh: &B256) -> String {
    format!("blob-{:02}_{}.bin", index, vh)
}

fn parse_blob_file_name(file_name: &str) -> Result<Option<(usize, B256)>> {
    let index_versioned_hash_opt_str = file_name
        .strip_prefix("blob-")
        .and_then(|s| s.strip_suffix(".bin"));
    if let Some(index_versioned_hash_str) = index_versioned_hash_opt_str {
        // TODO: Error handling
        let [index_str, versioned_hash_str] = index_versioned_hash_str
            .split("_")
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let index = usize::from_str(index_str)?;
        let versioned_hash = B256::from_str(versioned_hash_str)?;
        Ok(Some((index, versioned_hash)))
    } else {
        Ok(None)
    }
}

const DIR_LEVELS: usize = 3;

fn slot_dir_from(base: &PathBuf, slot: u32) -> PathBuf {
    let slot_hi = slot / 1_000_000;
    let slot_mid = (slot - slot_hi * 1_000_000) / 1_000;
    let slot_lo = slot - slot_hi * 1_000_000 - slot_mid * 1_000;
    let mut slot_dir = base.clone();
    slot_dir.push("by_slot");
    slot_dir.push(format!("{:03}", slot_hi));
    slot_dir.push(format!("{:03}", slot_mid));
    slot_dir.push(format!("{:03}", slot_lo));
    slot_dir
}

fn root_dir_from(base: &PathBuf, root: &B256) -> PathBuf {
    let root_str = format!("{}", root);
    let root_hi = &root_str[0..5];
    let root_mid = &root_str[5..8];
    let root_lo = &root_str[8..];
    let mut root_dir = base.clone();
    root_dir.push("by_root");
    root_dir.push(format!("{:03}", root_hi));
    root_dir.push(format!("{:03}", root_mid));
    root_dir.push(format!("{:03}", root_lo));
    root_dir
}

#[derive(Clone)]
pub struct Store {
    pub blobs_path: String,
}

impl Store {
    pub(crate) fn delete_block_data(&self, slot_path: &Path) -> Result<()> {
        if !fs::exists(slot_path)? {
            return Ok(());
        }
        let mut root_path = fs::canonicalize(slot_path)?; // Resolve symlink
        if fs::exists(&root_path)? {
            // Remove at highest intermediate directory that only has one entry
            for _level in 0..DIR_LEVELS - 1 {
                let mut root_mid_path = root_path.clone();
                root_mid_path.pop();
                if fs::read_dir(&root_mid_path)?.count() == 1 {
                    root_path = root_mid_path;
                } else {
                    break;
                }
            }
            info!("Removing stale dir recursively at {:?}", root_path);
            fs::remove_dir_all(&root_path)?;
        }
        info!("Removing stale symlink at {:?}", slot_path);
        fs::remove_file(slot_path)?;
        Ok(())
    }

    // Find the latest valid processed block header (and deleting stale ones if found)
    pub(crate) fn last_header(&self) -> Result<Option<BlockHeader>> {
        fn read_file(path: &Path) -> Result<Vec<u8>, io::Error> {
            let mut file = File::open(path)?;
            let mut data = Vec::new();
            file.read_to_end(&mut data)?;
            Ok(data)
        }
        'outer: loop {
            let mut slot_path = PathBuf::from(&self.blobs_path);
            slot_path.push("by_slot");
            // Find the highest slot number path
            for _level in 0..DIR_LEVELS {
                let read_dir = match fs::read_dir(&slot_path) {
                    Err(e) => match e.kind() {
                        io::ErrorKind::NotFound => break 'outer,
                        _ => return Err(e.into()),
                    },
                    Ok(entries) => entries,
                };
                let mut entries = read_dir
                    .map(|res| res.map(|e| e.path()))
                    .collect::<Result<Vec<_>, io::Error>>()?;
                entries.sort();
                let last = if let Some(last) = entries.last() {
                    last
                } else {
                    break 'outer;
                };
                // Next level
                slot_path.push(last);
            }
            // See if that block was completed, if not delete it and try again
            let mut header_path = slot_path.clone();
            header_path.push("header.json");
            let header_data = match read_file(&header_path) {
                Err(e) => match e.kind() {
                    io::ErrorKind::NotFound => {
                        // The node didn't complete processing this block, remove the stale data in
                        // reverse order of creation.
                        self.delete_block_data(&slot_path)?;
                        continue 'outer;
                    }
                    _ => return Err(e.into()),
                },
                Ok(data) => data,
            };
            let header: BlockHeader = serde_json::from_slice(&header_data)?;
            return Ok(Some(header));
        }
        // No block found
        Ok(None)
    }

    pub(crate) fn slot_dir(&self, slot: u32) -> PathBuf {
        let base = PathBuf::from(&self.blobs_path);
        slot_dir_from(&base, slot)
    }

    fn root_dir(&self, root: &B256) -> PathBuf {
        let base = PathBuf::from(&self.blobs_path);
        root_dir_from(&base, root)
    }

    // Returns a vector of index, versioned_hash, blob; sorted by index
    pub(crate) async fn load_blobs_disk(
        &self,
        root: &B256,
    ) -> Result<Vec<(usize, B256, HeapBlob)>> {
        let root_dir = self.root_dir(root);
        let rd = match read_dir(&root_dir) {
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    return Ok(Vec::new());
                } else {
                    return Err(e.into());
                }
            }
            Ok(rd) => rd,
        };
        info!("loading blobs of slot root {} from {:?}", root, root_dir);
        let mut blobs = Vec::new();
        for entry in rd {
            let entry = entry?;
            let file_name = entry.file_name();
            let file_name = file_name.to_str().unwrap_or("");
            if let Some((index, versioned_hash)) = parse_blob_file_name(file_name)? {
                let file_path = root_dir.join(file_name);
                let mut file = File::open(&file_path)?;
                let mut data = Vec::new();
                file.read_to_end(&mut data)?;
                blobs.push((
                    index,
                    versioned_hash,
                    HeapBlob::from_bytes(Bytes::from(data))?,
                ));
            }
        }
        blobs.sort_by_key(|(index, _, _)| *index);
        Ok(blobs)
    }

    async fn store_blobs_disk(
        &self,
        beacon_block_header: &BlockHeader,
        blobs: &[(usize, B256, HeapBlob)],
    ) -> Result<()> {
        let slot_path = self.slot_dir(beacon_block_header.slot);
        let root_dir = self.root_dir(&beacon_block_header.root);
        info!(
            "storing {} blobs of slot {} to {:?} with symlink {:?}",
            blobs.len(),
            beacon_block_header.slot,
            root_dir,
            slot_path
        );
        for (index, vh, blob) in blobs {
            let name = blob_file_name(*index, vh);
            let blob_path = root_dir.join(&name);
            let blob_path_tmp = root_dir.join(format!("{}.tmp", name));
            let mut file_tmp = File::create(&blob_path_tmp)?;
            file_tmp.write_all(blob.inner())?;
            rename(blob_path_tmp, blob_path)?;
        }
        Ok(())
    }
}

pub struct Node {
    pub beacon_cli: BeaconClient,
    pub rpc_cli: RootProvider,
    pub store: Store,
    pub config: Config,
}

impl Node {
    /// Construct network clients and bind shared state/sync stores.
    pub async fn new(config: Config) -> Result<Self> {
        let store = Store {
            blobs_path: config.blobs_path.clone(),
        };
        let http_cli = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .read_timeout(Duration::from_secs(8))
            .connect_timeout(Duration::from_secs(8))
            .build()?;

        let exp_backoff = Some(ExponentialBackoffBuilder::default().build());
        let beacon_cli_config = beacon::Config {
            base_url: config.beacon_url.clone(),
            exp_backoff,
        };
        let beacon_cli = BeaconClient::try_with_client(http_cli, beacon_cli_config)?;
        let rpc_cli = RootProvider::<Ethereum>::new_http(config.rpc_url.parse()?);

        Ok(Self {
            beacon_cli,
            rpc_cli,
            store,
            config,
        })
    }

    pub(crate) async fn process_beacon_block_header(
        &self,
        beacon_block_header: &BlockHeader,
    ) -> Result<()> {
        // Create the path with the symlink to the yet to be created block dir, indexed by slot
        let slot_path = self.store.slot_dir(beacon_block_header.slot);
        let mut slot_mid_path = slot_path.clone();
        slot_mid_path.pop();
        create_dir_all(&slot_mid_path)?;
        let root_dir_rel = root_dir_from(
            &["..", "..", ".."].iter().collect(),
            &beacon_block_header.root,
        );
        unix::fs::symlink(&root_dir_rel, slot_path)?;

        // Create the block dir where the filtered blobs and header will be stored, indexed by
        // block root
        let root_dir = self.store.root_dir(&beacon_block_header.root);
        create_dir_all(&root_dir)?;
        // We store the header as a tmp file, and only rename after successfully processing the
        // beacon block.  This way seeing the `header.json` without the `.tmp` guarantees that the
        // stored block data is valid.
        let mut header_path_tmp = root_dir.clone();
        header_path_tmp.push("header.json.tmp");
        let header_json = serde_json::to_vec(beacon_block_header)?;
        let mut file_tmp = File::create(&header_path_tmp)?;
        file_tmp.write_all(&header_json)?;

        self.process_beacon_block_blobs(beacon_block_header).await?;

        let mut header_path = root_dir.clone();
        header_path.push("header.json");
        rename(header_path_tmp, header_path)?;
        Ok(())
    }

    pub(crate) async fn process_beacon_block_blobs(
        &self,
        beacon_block_header: &BlockHeader,
    ) -> Result<()> {
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
                return Ok(());
            }
        };
        let execution_payload = match beacon_block.execution_payload {
            Some(payload) => payload,
            None => {
                debug!("slot {} has no execution payload", slot);
                return Ok(());
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

        let kzg_blob_commitments = match beacon_block.blob_kzg_commitments {
            Some(commitments) => commitments
                .into_iter()
                .map(|c| (kzg_to_versioned_hash(c.as_ref()), c))
                .collect(),
            None => Vec::new(),
        };
        if kzg_blob_commitments.is_empty() {
            debug!("slot {} has no blobs", slot);
            return Ok(());
        }

        let execution_block_hash = execution_payload.block_hash;

        let execution_block_id = alloy_eips::eip1898::BlockId::Hash(execution_block_hash.into());
        let execution_block = self
            .rpc_cli
            .get_block(execution_block_id)
            .full()
            .await?
            .with_context(|| format!("Execution block {execution_block_hash} not found"))?;

        let indexed_blob_txs: Vec<_> = match execution_block.transactions.as_transactions() {
            Some(txs) => txs
                .iter()
                .enumerate()
                .filter(|(_index, tx)| {
                    tx.inner.blob_versioned_hashes().is_some()
                        && tx.as_recovered().to() == Some(self.config.filter_address)
                })
                .collect(),
            None => {
                return Err(anyhow!(
                    "Consensus block {beacon_block_root} has blobs but the execution block doesn't have txs"
                ));
            }
        };

        let txs_blobs_vhs: Vec<B256> = indexed_blob_txs
            .iter()
            .flat_map(|(_, tx)| {
                tx.as_recovered()
                    .blob_versioned_hashes()
                    .expect("tx has blobs")
            })
            .cloned()
            .collect();

        info!(
            "slot {} has {} blobs, after filter {}",
            slot,
            kzg_blob_commitments.len(),
            indexed_blob_txs.len()
        );

        if txs_blobs_vhs.is_empty() {
            return Ok(());
        }

        let blobs = self
            .get_blobs(beacon_block_header, &kzg_blob_commitments, &txs_blobs_vhs)
            .await?;
        assert_eq!(blobs.len(), txs_blobs_vhs.len());

        Ok(())
    }

    async fn get_blobs(
        &self,
        beacon_block_header: &BlockHeader,
        block_kzg_blob_commitments: &[(B256, KzgCommitment)],
        versioned_hashes: &[B256],
    ) -> Result<Vec<(usize, B256, HeapBlob)>> {
        let blobs = self
            .store
            .load_blobs_disk(&beacon_block_header.root)
            .await?;
        let mut result = Vec::new();
        let mut missing_vhs = Vec::new();
        for vh in versioned_hashes {
            if let Some((index, vh, blob)) = blobs.iter().find(|(_, vh0, _)| vh0 == vh) {
                result.push((*index, *vh, blob.clone()));
            } else {
                missing_vhs.push(*vh);
            }
        }
        let mut missing_blobs = Vec::new();
        if !missing_vhs.is_empty() {
            let blobs = self
                .beacon_cli
                .get_blobs(beacon_block_header.root.into(), &missing_vhs)
                .await?;
            assert_eq!(blobs.len(), missing_vhs.len());
            debug!("got {} blobs from beacon_cli", blobs.len());
            for (vh, blob) in zip_eq(missing_vhs, blobs) {
                let index = block_kzg_blob_commitments
                    .iter()
                    .position(|(vh0, _)| *vh0 == vh)
                    .unwrap();
                missing_blobs.push((index, vh, blob));
            }
            self.store
                .store_blobs_disk(beacon_block_header, &missing_blobs)
                .await?;
            result.extend(missing_blobs.into_iter());
        }
        Ok(result)
    }
}
