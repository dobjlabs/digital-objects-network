use std::{collections::HashSet, sync::RwLockReadGuard};

use alloy::eips::eip4844::FIELD_ELEMENT_BYTES_USIZE;
use anyhow::{anyhow, Context, Result};
use tracing::info;

use super::Node;
use crate::clients::beacon::types::Blob;

#[derive(Debug)]
pub(super) struct State {
    pub(super) transactions: HashSet<String>,
    pub(super) nullifiers: HashSet<String>,
}

impl Node {
    pub(super) fn read_state(&self) -> Result<RwLockReadGuard<'_, State>> {
        self.state
            .read()
            .map_err(|e| anyhow!("state read lock poisoned: {e}"))
    }

    pub fn state_snapshot(&self) -> Result<(Vec<String>, Vec<String>)> {
        let state = self.read_state()?;
        Ok((
            state.transactions.iter().cloned().collect(),
            state.nullifiers.iter().cloned().collect(),
        ))
    }

    pub(super) fn log_current_state(&self) -> Result<()> {
        let state = self.read_state()?;
        info!(
            "current state: transactions={:?}, nullifiers={:?}, ",
            state.transactions, state.nullifiers,
        );
        Ok(())
    }

    // This processes the digital object blob.
    pub(super) async fn process_do_blob(
        &self,
        blob: &Blob,
        slot: u32,
        block_number: Option<u32>,
    ) -> Result<()> {
        let _bytes =
            bytes_from_simple_blob(blob.blob.inner()).context("Invalid byte encoding in blob")?;

        // TODO: process the blob bytes and update the state accordingly
        self.db.persist_transaction("", slot, block_number).await?;
        self.db.persist_nullifier("", slot, block_number).await?;

        let state = self.read_state()?;
        info!(
            "state update: transactions={:?}, nullifiers={:?}, ",
            state.transactions, state.nullifiers,
        );
        Ok(())
    }
}

/// Extracts bytes from a blob in the "simple" encoding.
fn bytes_from_simple_blob(blob_bytes: &[u8]) -> Result<Vec<u8>> {
    if blob_bytes.len() < 9 {
        return Err(anyhow!(
            "Invalid blob length {}; expected at least 9 bytes",
            blob_bytes.len()
        ));
    }
    if !blob_bytes.len().is_multiple_of(FIELD_ELEMENT_BYTES_USIZE) {
        return Err(anyhow!(
            "Invalid blob length {}; expected multiple of {}",
            blob_bytes.len(),
            FIELD_ELEMENT_BYTES_USIZE
        ));
    }

    // Blob = [0x00] ++ 8_BYTE_LEN ++ [0x00,...,0x00] ++ X.
    let data_len = u64::from_be_bytes(std::array::from_fn(|i| blob_bytes[1 + i])) as usize;

    // Sanity check: Blob must be able to accommodate the specified data length.
    let field_elements = blob_bytes.len() / FIELD_ELEMENT_BYTES_USIZE;
    if field_elements < 1 {
        return Err(anyhow!("Invalid blob length {}", blob_bytes.len()));
    }
    let max_data_len = (field_elements - 1) * (FIELD_ELEMENT_BYTES_USIZE - 1);
    if data_len > max_data_len {
        return Err(anyhow!(
            "Given blob of length {} cannot accommodate {} bytes.",
            blob_bytes.len(),
            data_len
        ));
    }

    Ok(blob_bytes
        .chunks(FIELD_ELEMENT_BYTES_USIZE)
        .skip(1)
        .flat_map(|chunk| chunk[1..].to_vec())
        .take(data_len)
        .collect())
}
