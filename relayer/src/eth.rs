use std::str::FromStr;

use alloy::{
    eips::eip4844::builder::{SidecarBuilder, SimpleCoder},
    network::{Ethereum, TransactionBuilder4844},
    primitives::{Address, B256, U256},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
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

/// Worker-facing Ethereum operations. Kept as a trait for test mocking.
#[async_trait]
pub trait EthGateway: Send + Sync {
    async fn submit_payload(&self, payload_bytes: &[u8]) -> Result<String>;
    async fn poll_receipt(&self, tx_hash: &str) -> Result<Option<ReceiptOutcome>>;
}

impl EthClient {
    /// Build signer + provider from runtime config.
    pub async fn new(cfg: &AppConfig) -> Result<Self> {
        let signer: PrivateKeySigner = cfg
            .private_key
            .parse()
            .map_err(|e| anyhow!("invalid RELAYER_PRIVATE_KEY: {e}"))?;
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
            rpc_url = %cfg.rpc_url,
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
    async fn submit_payload(&self, payload_bytes: &[u8]) -> Result<String> {
        info!(
            payload_bytes = payload_bytes.len(),
            "Preparing EIP-4844 transaction from relay payload"
        );
        let sidecar = SidecarBuilder::<SimpleCoder>::from_slice(payload_bytes)
            .build_4844()
            .map_err(|e| anyhow!("build blob sidecar: {e}"))?;

        let mut tx = TransactionRequest::default()
            .to(self.to)
            .from(self.from)
            .value(U256::ZERO)
            .with_blob_sidecar(sidecar);

        if let Some(max_fee_per_blob_gas) = self.max_fee_per_blob_gas {
            tx = tx.max_fee_per_blob_gas(max_fee_per_blob_gas);
        }

        debug!(
            from = %self.from,
            to = %self.to,
            max_fee_per_blob_gas = ?self.max_fee_per_blob_gas,
            "Sending Ethereum blob transaction"
        );
        let pending = self.provider.send_transaction(tx).await?;
        let tx_hash = format!("{:#x}", pending.tx_hash());
        info!(tx_hash = %tx_hash, "Ethereum blob transaction submitted");
        Ok(tx_hash)
    }

    /// Query receipt status for a previously broadcast transaction hash.
    async fn poll_receipt(&self, tx_hash: &str) -> Result<Option<ReceiptOutcome>> {
        let tx_hash = parse_tx_hash(tx_hash)?;
        debug!(tx_hash = %format!("{tx_hash:#x}"), "Querying Ethereum transaction receipt");
        let receipt = self.provider.get_transaction_receipt(tx_hash).await?;
        Ok(receipt.map(|r| ReceiptOutcome {
            success: r.status(),
            block_number: r.block_number,
        }))
    }
}

/// Parse and validate hex tx hash values from job storage/API payloads.
pub fn parse_tx_hash(value: &str) -> Result<B256> {
    B256::from_str(value).map_err(|e| anyhow!("invalid tx hash '{value}': {e}"))
}
