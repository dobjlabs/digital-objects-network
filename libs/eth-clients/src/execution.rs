use std::task::{Context, Poll};

use alloy::{
    network::Ethereum,
    providers::RootProvider,
    rpc::{
        client::ClientBuilder,
        json_rpc::{RequestPacket, ResponsePacket},
    },
    transports::{
        layers::{RateLimitRetryPolicy, RetryPolicy},
        TransportError, TransportFut,
    },
};
use backoff::ExponentialBackoff;
use tower::{Layer, Service};
use url::Url;

/// Connect to an execution node over HTTP, retrying rate-limited requests with `backoff`.
pub fn connect(url: Url, backoff: ExponentialBackoff) -> RootProvider<Ethereum> {
    let client = ClientBuilder::default()
        .layer(ExponentialBackoffLayer::new(backoff))
        .http(url);
    RootProvider::new(client)
}

/// Transport layer that retries the requests alloy's rate-limit policy deems retryable,
/// waiting as `backoff` dictates or as long as the provider asks.
#[derive(Clone)]
pub struct ExponentialBackoffLayer {
    backoff: ExponentialBackoff,
}

impl ExponentialBackoffLayer {
    pub fn new(backoff: ExponentialBackoff) -> Self {
        Self { backoff }
    }
}

impl<S> Layer<S> for ExponentialBackoffLayer {
    type Service = ExponentialBackoffService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ExponentialBackoffService {
            inner,
            backoff: self.backoff.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ExponentialBackoffService<S> {
    inner: S,
    backoff: ExponentialBackoff,
}

impl<S> Service<RequestPacket> for ExponentialBackoffService<S>
where
    S: Service<RequestPacket, Response = ResponsePacket, Error = TransportError>
        + Clone
        + Send
        + Sync
        + 'static,
    S::Future: Send + 'static,
{
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: RequestPacket) -> Self::Future {
        let inner = self.inner.clone();
        let backoff = self.backoff.clone();
        Box::pin(async move {
            backoff::future::retry_notify(
                backoff,
                || {
                    let mut inner = inner.clone();
                    let request = request.clone();
                    async move {
                        // A JSON-RPC error carried by a successful HTTP response arrives as Ok.
                        let error = match inner.call(request).await {
                            Ok(response) => match response.as_error() {
                                Some(error) => TransportError::ErrorResp(error.clone()),
                                None => return Ok(response),
                            },
                            Err(error) => error,
                        };
                        let policy = RateLimitRetryPolicy::default();
                        if !policy.should_retry(&error) {
                            return Err(backoff::Error::permanent(error));
                        }
                        match policy.backoff_hint(&error) {
                            Some(delay) => Err(backoff::Error::retry_after(error, delay)),
                            None => Err(backoff::Error::transient(error)),
                        }
                    }
                },
                |error, delay: std::time::Duration| {
                    tracing::warn!(
                        ?error,
                        "Execution RPC request failed. Retrying in {} seconds",
                        delay.as_secs()
                    );
                },
            )
            .await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::atomic::Ordering, time::Duration};

    use alloy::providers::Provider;

    use super::*;
    use crate::common::tests::rate_limited_server;

    #[tokio::test]
    async fn rate_limited_requests_are_retried() {
        let (url, requests) =
            rate_limited_server(2, r#"{"jsonrpc":"2.0","id":0,"result":"0x10"}"#).await;
        let backoff = ExponentialBackoff {
            initial_interval: Duration::from_millis(1),
            max_elapsed_time: Some(Duration::from_secs(5)),
            ..Default::default()
        };
        let block_number = connect(url, backoff)
            .get_block_number()
            .await
            .expect("the request succeeds once the rate limit lifts");
        assert_eq!(block_number, 16);
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }
}
