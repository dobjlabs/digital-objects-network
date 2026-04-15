use std::str::FromStr;

use alloy::consensus::Transaction;
use alloy::{
    eips::eip4844::builder::{SidecarBuilder, SimpleCoder},
    network::{Ethereum, TransactionBuilder4844},
    primitives::{Address, B256, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::{BlockNumberOrTag, TransactionRequest},
    signers::local::PrivateKeySigner,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tracing::{debug, info};

use crate::config::AppConfig;

/// Concrete Ethereum RPC gateway used by the relayer.
pub struct EthClient {
    provider: DynProvider<Ethereum>,
    from: Address,
    to: Address,
    max_fee_per_blob_gas: Option<u128>,
}

/// Hard startup guard: signer must have at least this many wei.
const MIN_SIGNER_BALANCE_WEI: u128 = 10_000_000_000_000_000; // 0.01 ETH

/// Minimal receipt projection used by worker state transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiptOutcome {
    pub success: bool,
    pub block_number: Option<u64>,
}

/// Current network fee estimates for EIP-1559 + EIP-4844 transactions.
#[derive(Debug, Clone, Copy)]
pub struct FeeEstimate {
    pub base_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
}

/// Fee caps extracted from a pending/queued transaction.
#[derive(Debug, Clone, Copy)]
pub struct PendingTxFees {
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
    pub max_fee_per_blob_gas: Option<u128>,
}

/// Explicit fee overrides for replacement (fee-bump) transactions.
/// Fields set to `None` are left for alloy's provider to auto-fill.
#[derive(Debug, Clone, Copy)]
pub struct FeeOverrides {
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
    pub max_fee_per_blob_gas: Option<u128>,
}

/// Worker-facing Ethereum operations. Kept as a trait for test mocking.
#[async_trait]
pub trait EthGateway: Send + Sync {
    async fn submit_payload(&self, payload_bytes: &[u8], nonce: u64) -> Result<String>;
    async fn poll_receipt(&self, tx_hash: &str) -> Result<Option<ReceiptOutcome>>;
    /// Get the next available nonce for the relayer signer address.
    async fn get_next_nonce(&self) -> Result<u64>;
    /// Fetch current network fee estimates from the latest block.
    async fn get_current_fees(&self) -> Result<FeeEstimate>;
    /// Fetch fee caps from a previously submitted (possibly pending) transaction.
    async fn get_pending_tx_fees(&self, tx_hash: &str) -> Result<Option<PendingTxFees>>;
    /// Submit a blob TX with explicit nonce and fee overrides (for RBF replacement).
    async fn submit_payload_with_fees(
        &self,
        payload_bytes: &[u8],
        nonce: u64,
        fees: &FeeOverrides,
    ) -> Result<String>;
}

impl EthClient {
    /// Build signer + provider from runtime config.
    pub async fn new(cfg: &AppConfig) -> Result<Self> {
        let signer: PrivateKeySigner = cfg
            .private_key
            .parse()
            .map_err(|e| anyhow!("invalid PRIVATE_KEY: {e}"))?;
        let from = signer.address();

        let provider = ProviderBuilder::new()
            .wallet(signer)
            .connect_http(cfg.rpc_url.parse()?)
            .erased();

        // Fail fast when the relayer account cannot plausibly pay blob tx fees.
        let min_balance = U256::from(MIN_SIGNER_BALANCE_WEI);
        let signer_balance = provider.get_balance(from).await?;
        if signer_balance < min_balance {
            return Err(anyhow!(
                "insufficient signer balance for relayer startup: address={from}, balance_wei={signer_balance}, min_required_wei={min_balance}"
            ));
        }

        let client = Self {
            provider,
            from,
            to: cfg.to_address,
            max_fee_per_blob_gas: cfg.max_fee_per_blob_gas,
        };

        info!(
            from = %client.from,
            to = %client.to,
            signer_balance_wei = %signer_balance,
            min_required_balance_wei = %min_balance,
            max_fee_per_blob_gas = ?client.max_fee_per_blob_gas,
            "Initialized Ethereum gateway"
        );

        Ok(client)
    }
}

#[async_trait]
impl EthGateway for EthClient {
    /// Build and broadcast an EIP-4844 blob transaction from payload bytes.
    async fn submit_payload(&self, payload_bytes: &[u8], nonce: u64) -> Result<String> {
        info!(
            payload_bytes = payload_bytes.len(),
            nonce, "Preparing EIP-4844 transaction from relay payload"
        );
        let sidecar = SidecarBuilder::<SimpleCoder>::from_slice(payload_bytes)
            .build_4844()
            .map_err(|e| anyhow!("build blob sidecar: {e}"))?;

        let mut tx = TransactionRequest::default()
            .to(self.to)
            .from(self.from)
            .value(U256::ZERO)
            .nonce(nonce)
            .with_blob_sidecar(sidecar);

        if let Some(max_fee_per_blob_gas) = self.max_fee_per_blob_gas {
            tx = tx.max_fee_per_blob_gas(max_fee_per_blob_gas);
        }

        debug!(
            from = %self.from,
            to = %self.to,
            nonce,
            max_fee_per_blob_gas = ?self.max_fee_per_blob_gas,
            "Sending Ethereum blob transaction"
        );
        let pending = self.provider.send_transaction(tx).await?;
        let tx_hash = format!("{:#x}", pending.tx_hash());
        info!(tx_hash = %tx_hash, nonce, "Ethereum blob transaction submitted");
        Ok(tx_hash)
    }

    async fn get_next_nonce(&self) -> Result<u64> {
        let count = self
            .provider
            .get_transaction_count(self.from)
            .pending()
            .await?;
        Ok(count)
    }

    async fn get_current_fees(&self) -> Result<FeeEstimate> {
        let block = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await?
            .ok_or_else(|| anyhow!("latest block not available"))?;

        let base_fee_per_gas = block
            .header
            .base_fee_per_gas
            .ok_or_else(|| anyhow!("latest block missing base_fee_per_gas"))?;

        let priority_fee = self.provider.get_max_priority_fee_per_gas().await?;

        Ok(FeeEstimate {
            base_fee_per_gas: base_fee_per_gas as u128,
            max_priority_fee_per_gas: priority_fee,
        })
    }

    async fn get_pending_tx_fees(&self, tx_hash: &str) -> Result<Option<PendingTxFees>> {
        let hash =
            B256::from_str(tx_hash).map_err(|e| anyhow!("invalid tx hash '{tx_hash}': {e}"))?;
        let tx = self.provider.get_transaction_by_hash(hash).await?;
        Ok(tx.map(|t| PendingTxFees {
            max_fee_per_gas: t.max_fee_per_gas(),
            max_priority_fee_per_gas: t.max_priority_fee_per_gas().unwrap_or(0),
            max_fee_per_blob_gas: t.max_fee_per_blob_gas(),
        }))
    }

    async fn submit_payload_with_fees(
        &self,
        payload_bytes: &[u8],
        nonce: u64,
        fees: &FeeOverrides,
    ) -> Result<String> {
        info!(
            payload_bytes = payload_bytes.len(),
            nonce,
            max_fee_per_gas = fees.max_fee_per_gas,
            max_priority_fee_per_gas = fees.max_priority_fee_per_gas,
            max_fee_per_blob_gas = ?fees.max_fee_per_blob_gas,
            "Preparing fee-bumped EIP-4844 transaction"
        );
        let sidecar = SidecarBuilder::<SimpleCoder>::from_slice(payload_bytes)
            .build_4844()
            .map_err(|e| anyhow!("build blob sidecar: {e}"))?;

        let mut tx = TransactionRequest::default()
            .to(self.to)
            .from(self.from)
            .value(U256::ZERO)
            .with_blob_sidecar(sidecar)
            .nonce(nonce)
            .max_fee_per_gas(fees.max_fee_per_gas)
            .max_priority_fee_per_gas(fees.max_priority_fee_per_gas);

        if let Some(blob_fee) = fees.max_fee_per_blob_gas {
            tx = tx.max_fee_per_blob_gas(blob_fee);
        }

        let pending = self.provider.send_transaction(tx).await?;
        let tx_hash = format!("{:#x}", pending.tx_hash());
        info!(tx_hash = %tx_hash, nonce, "Fee-bumped blob transaction submitted");
        Ok(tx_hash)
    }

    async fn get_next_nonce(&self) -> Result<u64> {
        let count = self.provider.get_transaction_count(self.from).await?;
        Ok(count)
    }

    async fn get_current_fees(&self) -> Result<FeeEstimate> {
        let block = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await?
            .ok_or_else(|| anyhow!("latest block not available"))?;

        let base_fee_per_gas = block
            .header
            .base_fee_per_gas
            .ok_or_else(|| anyhow!("latest block missing base_fee_per_gas"))?;

        let priority_fee = self.provider.get_max_priority_fee_per_gas().await?;

        Ok(FeeEstimate {
            base_fee_per_gas: base_fee_per_gas as u128,
            max_priority_fee_per_gas: priority_fee,
        })
    }

    async fn get_pending_tx_fees(&self, tx_hash: &str) -> Result<Option<PendingTxFees>> {
        let hash =
            B256::from_str(tx_hash).map_err(|e| anyhow!("invalid tx hash '{tx_hash}': {e}"))?;
        let tx = self.provider.get_transaction_by_hash(hash).await?;
        Ok(tx.map(|t| PendingTxFees {
            max_fee_per_gas: t.max_fee_per_gas(),
            max_priority_fee_per_gas: t.max_priority_fee_per_gas().unwrap_or(0),
            max_fee_per_blob_gas: t.max_fee_per_blob_gas(),
        }))
    }

    async fn submit_payload_with_fees(
        &self,
        payload_bytes: &[u8],
        nonce: u64,
        fees: &FeeOverrides,
    ) -> Result<String> {
        info!(
            payload_bytes = payload_bytes.len(),
            nonce,
            max_fee_per_gas = fees.max_fee_per_gas,
            max_priority_fee_per_gas = fees.max_priority_fee_per_gas,
            max_fee_per_blob_gas = ?fees.max_fee_per_blob_gas,
            "Preparing fee-bumped EIP-4844 transaction"
        );
        let sidecar = SidecarBuilder::<SimpleCoder>::from_slice(payload_bytes)
            .build_4844()
            .map_err(|e| anyhow!("build blob sidecar: {e}"))?;

        let mut tx = TransactionRequest::default()
            .to(self.to)
            .from(self.from)
            .value(U256::ZERO)
            .with_blob_sidecar(sidecar)
            .nonce(nonce)
            .max_fee_per_gas(fees.max_fee_per_gas)
            .max_priority_fee_per_gas(fees.max_priority_fee_per_gas);

        if let Some(blob_fee) = fees.max_fee_per_blob_gas {
            tx = tx.max_fee_per_blob_gas(blob_fee);
        }

        let pending = self.provider.send_transaction(tx).await?;
        let tx_hash = format!("{:#x}", pending.tx_hash());
        info!(tx_hash = %tx_hash, nonce, "Fee-bumped blob transaction submitted");
        Ok(tx_hash)
    }

    /// Query receipt status for a previously broadcast transaction hash.
    async fn poll_receipt(&self, tx_hash: &str) -> Result<Option<ReceiptOutcome>> {
        let tx_hash =
            B256::from_str(tx_hash).map_err(|e| anyhow!("invalid tx hash '{tx_hash}': {e}"))?;
        debug!(tx_hash = %format!("{tx_hash:#x}"), "Querying Ethereum transaction receipt");
        let receipt = self.provider.get_transaction_receipt(tx_hash).await?;
        Ok(receipt.map(|r| ReceiptOutcome {
            success: r.status(),
            block_number: r.block_number,
        }))
    }
}
