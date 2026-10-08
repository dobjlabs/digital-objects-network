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

use std::{fmt::Display, str::FromStr, time::Duration};

use backoff::ExponentialBackoff;
use reqwest::{header::RETRY_AFTER, Client, Response, StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tracing::trace;

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
    auth_token: Option<&str>,
    exp_backoff: Option<ExponentialBackoff>,
) -> Result<ExpectedResponse, ClientError> {
    let auth_token = auth_token.unwrap_or("");
    trace!(
        method = "GET",
        url = url.clone().as_str(),
        "Dispatching API request"
    );

    let mut req = client.get(url.clone());

    if !auth_token.is_empty() {
        req = req.bearer_auth(auth_token);
    }

    let resp = if let Some(e) = exp_backoff {
        match backoff::future::retry_notify(
            e,
            || {
                let req = req.try_clone().unwrap();
                let url = url.clone();

                async move {
                    let resp = req.send().await.map_err(ClientError::from)?;
                    match retryable_status(resp, url).await {
                        Ok(resp) => Ok(resp),
                        Err((err, Some(retry_after))) => {
                            Err(backoff::Error::retry_after(err, retry_after))
                        }
                        Err((err, None)) => Err(backoff::Error::transient(err)),
                    }
                }
            },
            |error, duration: std::time::Duration| {
                let duration = duration.as_secs();

                tracing::warn!(
                    method = "GET",
                    url = %url,
                    ?error,
                    "Request failed. Retrying in {duration} seconds…"
                );
            },
        )
        .await
        {
            Ok(resp) => resp,
            Err(error) => {
                tracing::warn!(
                    method = "GET",
                    url = %url,
                    ?error,
                    "Request failed. All retries failed"
                );

                return Err(error);
            }
        }
    } else {
        match req.send().await {
            Err(error) => {
                tracing::warn!(
                    method = "GET",
                    url = %url,
                    ?error,
                    "Failed to send request"
                );

                return Err(error.into());
            }
            Ok(resp) => retryable_status(resp, url.clone())
                .await
                .map_err(|(err, _retry_after)| err)?,
        }
    };

    let status = resp.status();

    if status.as_u16() == 404 {
        return Err(ClientError::NotFound(url));
    };

    let text = resp.text().await?;
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

/// Pass `resp` through unless its status is a transient server condition, in which case it
/// becomes an error, paired with the delay the server asked for when it named one in seconds.
async fn retryable_status(
    resp: Response,
    url: Url,
) -> Result<Response, (ClientError, Option<Duration>)> {
    let status = resp.status();
    let retryable = matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    );
    if !retryable {
        return Ok(resp);
    }
    let retry_after = resp
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let body = resp.text().await.unwrap_or_default();
    Err((
        ClientError::RetryableStatus { url, status, body },
        retry_after,
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::*;

    /// Serve `rate_limited` 429 answers, then a 200, one per connection; returns the base URL
    /// and the count of requests seen.
    async fn rate_limited_server(rate_limited: usize) -> (Url, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let response = if seen.fetch_add(1, Ordering::SeqCst) < rate_limited {
                    let body = r#"{"code":-32007,"message":"50/second request limit reached"}"#;
                    format!(
                        "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                } else {
                    let body = r#"{"ok":true}"#;
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        (url, requests)
    }

    #[tokio::test]
    async fn rate_limited_requests_are_retried() {
        let (url, requests) = rate_limited_server(2).await;
        let backoff = ExponentialBackoff {
            initial_interval: Duration::from_millis(1),
            max_elapsed_time: Some(Duration::from_secs(5)),
            ..Default::default()
        };
        let value: serde_json::Value = json_get(&Client::new(), url, None, Some(backoff))
            .await
            .expect("the request succeeds once the rate limit lifts");
        assert_eq!(value["ok"], true);
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn rate_limit_without_backoff_is_a_status_error() {
        let (url, _) = rate_limited_server(1).await;
        let err = json_get::<serde_json::Value>(&Client::new(), url, None, None)
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
