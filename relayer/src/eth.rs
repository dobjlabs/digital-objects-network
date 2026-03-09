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

use crate::config::AppConfig;

pub struct EthClient {
    provider: DynProvider<Ethereum>,
    from: Address,
    to: Address,
    max_fee_per_blob_gas: Option<u128>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiptOutcome {
    pub success: bool,
    pub block_number: Option<u64>,
}

#[async_trait]
pub trait EthGateway: Send + Sync {
    async fn submit_payload(&self, payload_bytes: &[u8]) -> Result<String>;
    async fn poll_receipt(&self, tx_hash: &str) -> Result<Option<ReceiptOutcome>>;
}

impl EthClient {
    pub fn new(cfg: &AppConfig) -> Result<Self> {
        let signer: PrivateKeySigner = cfg
            .private_key
            .parse()
            .map_err(|e| anyhow!("invalid RELAYER_PRIVATE_KEY: {e}"))?;
        let from = signer.address();

        let provider = ProviderBuilder::new()
            .wallet(signer)
            .connect_http(cfg.rpc_url.parse()?)
            .erased();

        Ok(Self {
            provider,
            from,
            to: cfg.to_address,
            max_fee_per_blob_gas: cfg.max_fee_per_blob_gas,
        })
    }

    async fn submit_payload_inner(&self, payload_bytes: &[u8]) -> Result<String> {
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

        let pending = self.provider.send_transaction(tx).await?;
        Ok(format!("{:#x}", pending.tx_hash()))
    }

    async fn poll_receipt_inner(&self, tx_hash: &str) -> Result<Option<ReceiptOutcome>> {
        let tx_hash = parse_tx_hash(tx_hash)?;
        let receipt = self.provider.get_transaction_receipt(tx_hash).await?;
        Ok(receipt.map(|r| ReceiptOutcome {
            success: r.status(),
            block_number: r.block_number,
        }))
    }
}

#[async_trait]
impl EthGateway for EthClient {
    async fn submit_payload(&self, payload_bytes: &[u8]) -> Result<String> {
        self.submit_payload_inner(payload_bytes).await
    }

    async fn poll_receipt(&self, tx_hash: &str) -> Result<Option<ReceiptOutcome>> {
        self.poll_receipt_inner(tx_hash).await
    }
}

pub fn parse_tx_hash(value: &str) -> Result<B256> {
    B256::from_str(value).map_err(|e| anyhow!("invalid tx hash '{value}': {e}"))
}
