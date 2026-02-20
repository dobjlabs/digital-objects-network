use std::{collections::HashSet, sync::RwLockReadGuard};

use anyhow::{anyhow, Context, Result};
use synchronizer::{bytes_from_simple_blob, clients::beacon::types::Blob};
use tracing::info;

use super::Node;

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
