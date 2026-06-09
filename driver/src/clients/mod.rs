use std::time::Duration;

mod relayer_client;
mod synchronizer_client;

#[cfg(test)]
pub(crate) use relayer_client::RelayerConfirmation;
pub(crate) use relayer_client::{HttpRelayerClient, RelayerClient};
pub use relayer_client::{
    RELAYER_CONFIRM_TIMEOUT_SECS, RELAYER_POLL_INTERVAL_MS, RELAYER_TX_HASH_TIMEOUT_SECS,
};
pub(crate) use synchronizer_client::{HttpSynchronizerClient, SynchronizerClient};
pub use synchronizer_client::{SYNCHRONIZER_POLL_INTERVAL_MS, SYNCHRONIZER_POLL_TIMEOUT_SECS};
#[cfg(test)]
pub(crate) use synchronizer_client::{SynchronizerHead, SynchronizerMembership};

/// Per-request ceiling for a single relayer/synchronizer HTTP call. The poll
/// loops only re-check their overall deadline between iterations, so without a
/// per-request timeout one stalled `send()` blocks past the deadline forever.
/// Bounding each attempt makes the loop deadlines actually enforceable.
const HTTP_REQUEST_TIMEOUT_SECS: u64 = 30;
/// Cap on connection establishment, so an unreachable host fails fast instead
/// of waiting out the full request timeout on every poll iteration.
const HTTP_CONNECT_TIMEOUT_SECS: u64 = 10;

/// Build a blocking HTTP client with bounded connect + total request time,
/// reused across calls so the connection pool survives between polls.
pub(crate) fn build_http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(HTTP_CONNECT_TIMEOUT_SECS))
        .timeout(Duration::from_secs(HTTP_REQUEST_TIMEOUT_SECS))
        .build()
        .expect("failed to build blocking HTTP client")
}

pub(crate) fn is_retryable_request_error(err: &anyhow::Error) -> bool {
    err.chain()
        .filter_map(|cause| cause.downcast_ref::<reqwest::Error>())
        .any(|err| err.is_timeout() || err.is_connect() || err.is_body())
}
