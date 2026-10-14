// MIT License Copyright (c) 2022 Blobscan <https://blobscan.com>
//
// Permission is hereby granted, free of charge,
// to any person obtaining a copy of this software and associated documentation
// files (the "Software"), to deal in the Software without restriction, including
// without limitation the rights to use, copy, modify, merge, publish, distribute,
// sublicense, and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above
// copyright notice and this permission notice (including the next paragraph) shall
// be included in all copies or substantial portions of the Software.
//
// THE SOFTWARE
// IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
// BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
// PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS
// BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF
// CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
// SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

use std::{
    fmt::{Debug, Display},
    future::Future,
    str::FromStr,
    time::Duration,
};

use backoff::{backoff::Backoff, ExponentialBackoff};
use reqwest::{header::RETRY_AFTER, Client, RequestBuilder, StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tracing::{trace, warn};

/// Longest delay honoured when a server says how long to wait before retrying.
const MAX_SERVER_RETRY_DELAY: Duration = Duration::from_mins(1);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum NumericOrTextCode {
    String(String),
    /// Signed because JSON-RPC style providers answer with negative codes.
    Number(i64),
}
/// API Error response
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ErrorResponse {
    /// Error code
    pub code: NumericOrTextCode,
    /// Error message
    #[serde(default)]
    pub message: Option<String>,
}

impl ErrorResponse {
    /// The body a server must return for a non-404 failure. [`json_get`] parses
    /// every such response as a [`ClientResponse`], so a body that is not this
    /// shape reaches the caller as a deserialization error with the real
    /// message dropped.
    pub fn new(code: u16, message: impl Into<String>) -> Self {
        Self {
            code: NumericOrTextCode::Number(code.into()),
            message: Some(message.into()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Reqwest Error
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),

    /// API Error
    #[error("API usage error: {0}")]
    ApiError(ErrorResponse),

    /// Other Error
    #[error(transparent)]
    Other(#[from] anyhow::Error),

    /// Url Parsing Error
    #[error("{0}")]
    UrlParse(#[from] url::ParseError),

    /// Serde Json deser Error
    #[error("{0}")]
    SerdeError(#[from] serde_json::Error),

    /// NotFound (status 404)
    #[error("NotFound: {0}")]
    NotFound(Url),

    #[error("Empty response")]
    Empty,

    /// A status worth retrying, returned once the retries are exhausted or when retrying is off
    #[error("{url} answered with status {status}: {body}")]
    RetryableStatus {
        url: Url,
        status: StatusCode,
        body: String,
    },
}

/// API Response
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum ClientResponse<T> {
    /// Error
    Error(ErrorResponse),
    /// Success w/ value
    Success(T),
    /// Empty Success
    EmptySuccess,
}

pub type ClientResult<T> = Result<T, ClientError>;

impl<T> ClientResponse<T> {
    pub(crate) fn into_client_result(self) -> ClientResult<T> {
        match self {
            ClientResponse::Error(e) => Err(e.into()),
            ClientResponse::Success(t) => Ok(t),
            ClientResponse::EmptySuccess => Err(ClientError::Empty),
        }
    }

    #[allow(dead_code)]
    /// True if the response is an API error
    pub fn is_err(&self) -> bool {
        matches!(self, Self::Error(_))
    }
}

impl<T> FromStr for ClientResponse<T>
where
    T: serde::de::DeserializeOwned,
{
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(ClientResponse::EmptySuccess);
        }
        serde_json::from_str(s)
    }
}

impl From<ErrorResponse> for ClientError {
    fn from(err: ErrorResponse) -> Self {
        Self::ApiError(err)
    }
}

impl Display for NumericOrTextCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::String(s) => f.write_str(s.to_string().as_ref()),
            Self::Number(n) => f.write_str(n.to_string().as_ref()),
        }
    }
}
impl Display for ErrorResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&format!(
            "Code: {}, Message: \"{}\"",
            self.code,
            self.message.as_deref().unwrap_or(""),
        ))
    }
}

pub(crate) async fn json_get<ExpectedResponse: DeserializeOwned>(
    client: &Client,
    url: Url,
    exp_backoff: ExponentialBackoff,
) -> Result<ExpectedResponse, ClientError> {
    trace!(
        method = "GET",
        url = url.clone().as_str(),
        "Dispatching API request"
    );

    let req = client.get(url.clone());

    let (status, text) = retry(exp_backoff, url.as_str(), || {
        fetch_text(
            req.try_clone().expect("a GET request has no streamed body"),
            url.clone(),
        )
    })
    .await?;

    if status == StatusCode::NOT_FOUND {
        return Err(ClientError::NotFound(url));
    };

    let result: Result<ClientResponse<ExpectedResponse>, _> =
        serde_json::from_str(&text).map_err(ClientError::from);

    match result {
        Err(e) => {
            tracing::warn!(
                method = "GET",
                url = %url,
                status = status.as_u16(),
                body = %text,
                "Unexpected response from server"
            );

            Err(e)
        }
        Ok(response) => response.into_client_result(),
    }
}

/// Send `req` and read the whole body, so that a connection lost while reading is retried
/// like one lost while sending.
async fn fetch_text(
    req: RequestBuilder,
    url: Url,
) -> Result<(StatusCode, String), FailedAttempt<ClientError>> {
    let resp = req.send().await.map_err(FailedAttempt::from_reqwest)?;
    let status = resp.status();
    let retry_after = resp
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let text = resp.text().await.map_err(FailedAttempt::from_reqwest)?;
    if is_retryable_status(status.as_u16()) {
        return Err(FailedAttempt {
            error: ClientError::RetryableStatus {
                url,
                status,
                body: text,
            },
            retryable: true,
            retry_after,
        });
    }
    Ok((status, text))
}

/// HTTP statuses of transient server conditions, worth retrying.
pub(crate) fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 502 | 503 | 504)
}

/// Whether `err` means the exchange broke off -- no connection, a reset, or a timeout -- rather
/// than the request being invalid.
pub(crate) fn is_transient_reqwest_error(err: &reqwest::Error) -> bool {
    err.is_connect() || err.is_timeout() || err.is_request() || err.is_body()
}

/// One failed attempt of a request.
pub(crate) struct FailedAttempt<E> {
    pub(crate) error: E,
    pub(crate) retryable: bool,
    /// How long the server asked to wait before retrying.
    pub(crate) retry_after: Option<Duration>,
}

impl FailedAttempt<ClientError> {
    fn from_reqwest(err: reqwest::Error) -> Self {
        Self {
            retryable: is_transient_reqwest_error(&err),
            error: err.into(),
            retry_after: None,
        }
    }
}

/// Run `attempt` until it succeeds, fails in a way that is not retryable, or `backoff` gives up.
/// A retry waits as long as the server asked,
/// up to [`MAX_SERVER_RETRY_DELAY`], or else the next `backoff` interval. `backoff` is consulted
/// either way, so its elapsed-time limit always ends the retries.
pub(crate) async fn retry<T, E, Fut>(
    mut backoff: ExponentialBackoff,
    target: &str,
    mut attempt: impl FnMut() -> Fut,
) -> Result<T, E>
where
    E: Debug,
    Fut: Future<Output = Result<T, FailedAttempt<E>>>,
{
    backoff.reset();
    loop {
        let failed = match attempt().await {
            Ok(value) => return Ok(value),
            Err(failed) => failed,
        };
        if !failed.retryable {
            return Err(failed.error);
        }
        let Some(backoff_delay) = backoff.next_backoff() else {
            warn!(target, error = ?failed.error, "Request failed. All retries failed");
            return Err(failed.error);
        };
        let delay = failed
            .retry_after
            .map_or(backoff_delay, |delay| delay.min(MAX_SERVER_RETRY_DELAY));
        warn!(
            target,
            error = ?failed.error,
            "Request failed. Retrying in {} seconds",
            delay.as_secs()
        );
        tokio::time::sleep(delay).await;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::*;

    /// How the test server answers one connection.
    #[derive(Clone, Copy)]
    pub(crate) enum Reply {
        RateLimited,
        BadGateway,
        /// Close the connection without answering.
        Drop,
        /// Hold the connection open without answering, past any test client's timeout.
        Stall,
    }

    /// Answer the connections in order with `script`, then with 200s carrying `success_body`;
    /// returns the base URL and the count of requests seen.
    pub(crate) async fn scripted_server(
        script: Vec<Reply>,
        success_body: &'static str,
    ) -> (Url, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let reply = script.get(seen.fetch_add(1, Ordering::SeqCst)).copied();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = socket.read(&mut buf).await;
                    let (status, body) = match reply {
                        Some(Reply::RateLimited) => (
                            "429 Too Many Requests\r\nRetry-After: 0",
                            r#"{"code":-32007,"message":"50/second request limit reached"}"#,
                        ),
                        Some(Reply::BadGateway) => ("502 Bad Gateway", "bad gateway"),
                        Some(Reply::Drop) => return,
                        Some(Reply::Stall) => {
                            tokio::time::sleep(Duration::from_secs(30)).await;
                            return;
                        }
                        None => ("200 OK", success_body),
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        (url, requests)
    }

    pub(crate) fn fast_backoff() -> ExponentialBackoff {
        ExponentialBackoff {
            initial_interval: Duration::from_millis(1),
            max_elapsed_time: Some(Duration::from_secs(5)),
            ..Default::default()
        }
    }

    /// A client that gives up on a stalled connection quickly.
    pub(crate) fn impatient_client() -> Client {
        Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn rate_limited_requests_are_retried() {
        let (url, requests) = scripted_server(
            vec![Reply::RateLimited, Reply::RateLimited],
            r#"{"ok":true}"#,
        )
        .await;
        let value: serde_json::Value = json_get(&Client::new(), url, fast_backoff())
            .await
            .expect("the request succeeds once the rate limit lifts");
        assert_eq!(value["ok"], true);
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn broken_connections_and_gateway_errors_are_retried() {
        let script = vec![Reply::Drop, Reply::Stall, Reply::BadGateway];
        let (url, requests) = scripted_server(script, r#"{"ok":true}"#).await;
        let value: serde_json::Value = json_get(&impatient_client(), url, fast_backoff())
            .await
            .expect("the request succeeds once the server recovers");
        assert_eq!(value["ok"], true);
        assert_eq!(requests.load(Ordering::SeqCst), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn server_retry_delay_is_capped() {
        let started = tokio::time::Instant::now();
        let mut attempts = 0;
        let result: Result<(), &str> = retry(ExponentialBackoff::default(), "test", || {
            attempts += 1;
            let first_attempt = attempts == 1;
            async move {
                if first_attempt {
                    Err(FailedAttempt {
                        error: "busy",
                        retryable: true,
                        retry_after: Some(Duration::from_secs(3600)),
                    })
                } else {
                    Ok(())
                }
            }
        })
        .await;
        assert_eq!(result, Ok(()));
        assert_eq!(started.elapsed(), MAX_SERVER_RETRY_DELAY);
    }

    #[tokio::test]
    async fn exhausted_retries_return_the_status_error() {
        let (url, _) = scripted_server(vec![Reply::RateLimited], r#"{"ok":true}"#).await;
        let backoff = ExponentialBackoff {
            max_elapsed_time: Some(Duration::ZERO),
            ..Default::default()
        };
        let err = json_get::<serde_json::Value>(&Client::new(), url, backoff)
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                ClientError::RetryableStatus {
                    status: StatusCode::TOO_MANY_REQUESTS,
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn negative_json_rpc_code_parses_as_an_error() {
        let body = r#"{"code":-32007,"message":"50/second request limit reached"}"#;
        let parsed: ClientResponse<serde_json::Value> = serde_json::from_str(body).unwrap();
        match parsed {
            ClientResponse::Error(err) => assert_eq!(err.code.to_string(), "-32007"),
            other => panic!("expected an error response, got {other:?}"),
        }
    }
}
