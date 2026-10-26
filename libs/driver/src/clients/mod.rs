use std::time::Duration;

mod relayer_client;
mod synchronizer_client;

pub(crate) use relayer_client::{HttpRelayerClient, RelayerClient};
pub(crate) use synchronizer_client::{HttpSynchronizerClient, SynchronizerClient};
#[cfg(test)]
pub(crate) use synchronizer_client::{SynchronizerHead, SynchronizerMembership};

/// Total timeout for each relayer or synchronizer request.
const HTTP_REQUEST_TIMEOUT_SECS: u64 = 30;
/// Connection timeout for relayer and synchronizer requests.
const HTTP_CONNECT_TIMEOUT_SECS: u64 = 10;

pub(crate) fn build_http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(HTTP_CONNECT_TIMEOUT_SECS))
        .timeout(Duration::from_secs(HTTP_REQUEST_TIMEOUT_SECS))
        .build()
        .expect("failed to build blocking HTTP client")
}
