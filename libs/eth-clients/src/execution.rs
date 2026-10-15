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
        RpcError, TransportError, TransportErrorKind, TransportFut,
    },
};
use backoff::ExponentialBackoff;
use tower::{Layer, Service};
use url::Url;

use crate::common::{is_retryable_status, is_transient_reqwest_error, retry, FailedAttempt};

/// Connect to an execution node over HTTP through `http_client`, retrying transient failures
/// with `backoff`. Requests time out only if `http_client` sets a timeout.
pub fn connect(
    http_client: reqwest::Client,
    url: Url,
    backoff: ExponentialBackoff,
) -> RootProvider<Ethereum> {
    let client = ClientBuilder::default()
        .layer(ExponentialBackoffLayer::new(backoff))
        .http_with_client(http_client, url);
    RootProvider::new(client)
}

/// Transport layer that retries transient failures following an exponential backoff.
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
            retry(backoff, "execution RPC", || {
                let mut inner = inner.clone();
                let request = request.clone();
                async move {
                    match inner.call(request).await {
                        // A JSON-RPC error carried by a successful HTTP response arrives as Ok,
                        // and is passed on as such unless it is worth retrying.
                        Ok(response) => {
                            let Some(error) = response.as_error() else {
                                return Ok(response);
                            };
                            let failed = failed_attempt(TransportError::ErrorResp(error.clone()));
                            if failed.retryable {
                                Err(failed)
                            } else {
                                Ok(response)
                            }
                        }
                        Err(error) => Err(failed_attempt(error)),
                    }
                }
            })
            .await
        })
    }
}

/// Classify a failed request: an HTTP status or a broken connection is judged like a beacon
/// request; anything else, such as a rate limit reported in a JSON-RPC error, by alloy's policy.
fn failed_attempt(error: TransportError) -> FailedAttempt<TransportError> {
    let policy = RateLimitRetryPolicy::default();
    let retryable = match &error {
        RpcError::Transport(TransportErrorKind::HttpError(http_error)) => {
            is_retryable_status(http_error.status)
        }
        RpcError::Transport(TransportErrorKind::Custom(custom)) => custom
            .downcast_ref::<reqwest::Error>()
            .is_some_and(is_transient_reqwest_error),
        _ => policy.should_retry(&error),
    };
    FailedAttempt {
        retry_after: policy.backoff_hint(&error),
        retryable,
        error,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use alloy::providers::Provider;

    use super::*;
    use crate::common::tests::{fast_backoff, impatient_client, scripted_server, Reply};

    const BLOCK_NUMBER_RESPONSE: &str = r#"{"jsonrpc":"2.0","id":0,"result":"0x10"}"#;

    #[tokio::test]
    async fn rate_limited_requests_are_retried() {
        let script = vec![Reply::RateLimited, Reply::RateLimited];
        let (url, requests) = scripted_server(script, BLOCK_NUMBER_RESPONSE).await;
        let block_number = connect(reqwest::Client::new(), url, fast_backoff())
            .get_block_number()
            .await
            .expect("the request succeeds once the rate limit lifts");
        assert_eq!(block_number, 16);
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn broken_connections_and_gateway_errors_are_retried() {
        let script = vec![Reply::Drop, Reply::Stall, Reply::BadGateway];
        let (url, requests) = scripted_server(script, BLOCK_NUMBER_RESPONSE).await;
        let block_number = connect(impatient_client(), url, fast_backoff())
            .get_block_number()
            .await
            .expect("the request succeeds once the server recovers");
        assert_eq!(block_number, 16);
        assert_eq!(requests.load(Ordering::SeqCst), 4);
    }
}
