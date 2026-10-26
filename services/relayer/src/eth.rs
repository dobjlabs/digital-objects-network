use std::str::FromStr;

use alloy::consensus::Transaction;
use alloy::eips::Encodable2718;
use alloy::{
    eips::eip4844::builder::{SidecarBuilder, SimpleCoder},
    network::{Ethereum, EthereumWallet, TransactionBuilder, TransactionBuilder4844},
    primitives::{Address, B256, U256},
    providers::{
        fillers::{FillProvider, RecommendedFillers},
        DynProvider, Provider, ProviderBuilder,
    },
    rpc::types::{BlockNumberOrTag, TransactionRequest},
    signers::local::PrivateKeySigner,
};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use tracing::{debug, info};

use crate::config::AppConfig;

/// Concrete Ethereum RPC gateway used by the relayer.
pub struct EthClient {
    provider: DynProvider<Ethereum>,
    from: Address,
    to: Address,
    max_fee_per_blob_gas: Option<u128>,
    wallet: EthereumWallet,
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

/// A signed transaction that can be persisted before network submission.
#[derive(Debug, Clone)]
pub struct PreparedTransaction {
    pub tx_hash: String,
    pub nonce: u64,
    pub raw_transaction: Vec<u8>,
}

#[derive(Debug)]
pub enum BroadcastError {
    RpcRefusal(anyhow::Error),
    Uncertain(anyhow::Error),
}

/// Worker-facing Ethereum operations. Kept as a trait for test mocking.
#[async_trait]
pub trait EthGateway: Send + Sync {
    /// Fill and sign without submitting to the network.
    async fn prepare_payload(
        &self,
        payload_bytes: &[u8],
        nonce: u64,
    ) -> Result<PreparedTransaction>;
    async fn broadcast(&self, transaction: &PreparedTransaction) -> Result<(), BroadcastError>;
    async fn poll_receipt(&self, tx_hash: &str) -> Result<Option<ReceiptOutcome>>;
    /// Get the next available nonce for the relayer signer address.
    async fn get_next_nonce(&self) -> Result<u64>;
    /// Fetch current network fee estimates from the latest block.
    async fn get_current_fees(&self) -> Result<FeeEstimate>;
    /// Fetch fee caps from a previously submitted (possibly pending) transaction.
    async fn get_pending_tx_fees(&self, tx_hash: &str) -> Result<Option<PendingTxFees>>;
    /// Prepare a replacement with explicit nonce and fee overrides without broadcasting it.
    async fn prepare_payload_with_fees(
        &self,
        payload_bytes: &[u8],
        nonce: u64,
        fees: &FeeOverrides,
    ) -> Result<PreparedTransaction>;
}

impl EthClient {
    /// Build signer + provider from runtime config.
    pub async fn new(cfg: &AppConfig) -> Result<Self> {
        let signer: PrivateKeySigner = cfg
            .private_key
            .parse()
            .map_err(|e| anyhow!("invalid PRIVATE_KEY: {e}"))?;
        let from = signer.address();
        let wallet = EthereumWallet::from(signer);

        let provider = ProviderBuilder::new()
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
            wallet,
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

    async fn prepare_request(&self, request: TransactionRequest) -> Result<PreparedTransaction> {
        let filled = FillProvider::new(self.provider.clone(), Ethereum::recommended_fillers())
            .fill(request)
            .await?;
        let request = filled
            .as_builder()
            .context("transaction unexpectedly signed during filling")?;
        let envelope = request.clone().build(&self.wallet).await?;
        Ok(PreparedTransaction {
            tx_hash: format!("{:#x}", envelope.tx_hash()),
            nonce: envelope.nonce(),
            raw_transaction: envelope.encoded_2718(),
        })
    }
}

#[async_trait]
impl EthGateway for EthClient {
    /// Fill and sign an EIP-4844 transaction without broadcasting it.
    async fn prepare_payload(
        &self,
        payload_bytes: &[u8],
        nonce: u64,
    ) -> Result<PreparedTransaction> {
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
            "Preparing Ethereum blob transaction"
        );
        self.prepare_request(tx).await
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

    async fn prepare_payload_with_fees(
        &self,
        payload_bytes: &[u8],
        nonce: u64,
        fees: &FeeOverrides,
    ) -> Result<PreparedTransaction> {
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

        self.prepare_request(tx).await
    }

    async fn broadcast(&self, transaction: &PreparedTransaction) -> Result<(), BroadcastError> {
        let pending = match self
            .provider
            .send_raw_transaction(&transaction.raw_transaction)
            .await
        {
            Ok(pending) => pending,
            Err(error)
                if error.as_error_resp().is_some_and(|reply| {
                    reply.message.to_ascii_lowercase().contains("already known")
                }) =>
            {
                return Ok(())
            }
            Err(error) if error.as_error_resp().is_some() => {
                return Err(BroadcastError::RpcRefusal(error.into()));
            }
            Err(error) => return Err(BroadcastError::Uncertain(error.into())),
        };
        if format!("{:#x}", pending.tx_hash()) != transaction.tx_hash {
            return Err(BroadcastError::Uncertain(anyhow!(
                "RPC returned a different transaction hash"
            )));
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        consensus::{transaction::SignerRecoverable, TxEnvelope},
        eips::Decodable2718,
    };

    #[tokio::test]
    async fn prepares_recoverable_blob_transaction_without_broadcasting() -> Result<()> {
        let signer: PrivateKeySigner = format!("{:064x}", 1).parse()?;
        let from = signer.address();
        let client = EthClient {
            provider: ProviderBuilder::new()
                .connect_http("http://127.0.0.1:9".parse()?)
                .erased(),
            from,
            to: Address::repeat_byte(7),
            max_fee_per_blob_gas: Some(2),
            wallet: EthereumWallet::from(signer),
        };
        let sidecar = SidecarBuilder::<SimpleCoder>::from_slice(b"fixture").build_4844()?;
        let mut request = TransactionRequest::default()
            .from(from)
            .to(client.to)
            .nonce(7)
            .gas_limit(100_000)
            .max_fee_per_gas(100)
            .max_priority_fee_per_gas(1)
            .max_fee_per_blob_gas(2)
            .with_blob_sidecar(sidecar);
        request.chain_id = Some(1);
        let prepared = client.prepare_request(request).await?;
        let mut bytes = prepared.raw_transaction.as_slice();
        let decoded = TxEnvelope::decode_2718(&mut bytes)?;
        assert!(bytes.is_empty());
        assert_eq!(decoded.nonce(), 7);
        assert_eq!(prepared.nonce, decoded.nonce());
        assert_eq!(prepared.tx_hash, format!("{:#x}", decoded.tx_hash()));
        assert_eq!(decoded.recover_signer()?, from);
        assert_eq!(decoded.blob_versioned_hashes().unwrap().len(), 1);
        assert_eq!(
            decoded
                .as_eip4844()
                .unwrap()
                .tx()
                .sidecar()
                .unwrap()
                .blobs()
                .len(),
            1
        );
        assert_eq!(decoded.encoded_2718(), prepared.raw_transaction);
        Ok(())
    }

    #[tokio::test]
    async fn raw_broadcast_distinguishes_acknowledgment_refusal_and_uncertainty() -> Result<()> {
        use axum::{routing::post, Json, Router};
        use serde_json::{json, Value};
        use std::{
            collections::VecDeque,
            sync::{Arc, Mutex},
        };

        let hash = B256::repeat_byte(1);
        let responses = Arc::new(Mutex::new(VecDeque::from([
            json!({"result": hash}),
            json!({"error": {"code": -32000, "message": "already known"}}),
            json!({"result": B256::repeat_byte(2)}),
            json!({"error": {"code": -32000, "message": "nonce too low"}}),
        ])));
        let replies = Arc::clone(&responses);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let rpc_url = format!("http://{}", listener.local_addr()?);
        let router = Router::new().route(
            "/",
            post(move |Json(request): Json<Value>| {
                let replies = Arc::clone(&replies);
                async move {
                    assert_eq!(request["method"], "eth_sendRawTransaction");
                    assert_eq!(request["params"], json!(["0x010203"]));
                    let mut reply = replies.lock().unwrap().pop_front().unwrap();
                    reply["jsonrpc"] = json!("2.0");
                    reply["id"] = request["id"].clone();
                    Json(reply)
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let signer: PrivateKeySigner = format!("{:064x}", 1).parse()?;
        let client = EthClient {
            provider: ProviderBuilder::new()
                .connect_http(rpc_url.parse()?)
                .erased(),
            from: signer.address(),
            to: Address::ZERO,
            max_fee_per_blob_gas: None,
            wallet: EthereumWallet::from(signer),
        };
        let prepared = PreparedTransaction {
            tx_hash: format!("{hash:#x}"),
            nonce: 0,
            raw_transaction: vec![1, 2, 3],
        };
        assert!(client.broadcast(&prepared).await.is_ok());
        assert!(client.broadcast(&prepared).await.is_ok());
        let mismatch = client.broadcast(&prepared).await.unwrap_err();
        assert!(matches!(mismatch, BroadcastError::Uncertain(error)
            if error.to_string().contains("different transaction hash")));
        assert!(matches!(
            client.broadcast(&prepared).await,
            Err(BroadcastError::RpcRefusal(_))
        ));
        assert!(responses.lock().unwrap().is_empty());
        server.abort();
        let unavailable = EthClient {
            provider: ProviderBuilder::new()
                .connect_http("http://127.0.0.1:0".parse()?)
                .erased(),
            ..client
        };
        assert!(matches!(
            unavailable.broadcast(&prepared).await,
            Err(BroadcastError::Uncertain(_))
        ));
        Ok(())
    }
}
