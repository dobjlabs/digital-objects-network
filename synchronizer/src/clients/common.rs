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

use std::{fmt::Display, str::FromStr};

use backoff::ExponentialBackoff;
use reqwest::{Client, Url};
use serde::{de::DeserializeOwned, Deserialize};
use tracing::trace;

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum NumericOrTextCode {
    String(String),
    Number(usize),
}
/// API Error response
#[derive(Deserialize, Debug, Clone)]
pub struct ErrorResponse {
    /// Error code
    pub code: NumericOrTextCode,
    /// Error message
    #[serde(default)]
    pub message: Option<String>,
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

                async move { req.send().await.map_err(|err| err.into()) }
            },
            |error, duration: std::time::Duration| {
                let duration = duration.as_secs();

                tracing::warn!(
                    method = "GET",
                    url = %url,
                    ?error,
                    "Failed to send request. Retrying in {duration} seconds…"
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
                    "Failed to send request. All retries failed"
                );

                return Err(error.into());
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
            Ok(resp) => resp,
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
                // response = format!("{:?}", text.map(|t| t.as_str())),
                "Unexpected response from server"
            );

            Err(e)
        }
        Ok(response) => response.into_client_result(),
    }
}
