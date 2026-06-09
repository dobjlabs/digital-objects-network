use anyhow::{Context, Result, anyhow};
use reqwest::Client;
use serde::{Serialize, de::DeserializeOwned};

/// Thin HTTP client around dobjd's REST API. Constructed once per CLI
/// invocation; reqwest's connection pool handles keepalive.
#[derive(Clone)]
pub struct DobjdClient {
    base: String,
    http: Client,
}

impl DobjdClient {
    pub fn new(base: String) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            http: Client::new(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.url(path);
        let res = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        decode_json(res).await
    }

    pub async fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = self.url(path);
        let res = self
            .http
            .post(&url)
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        decode_json(res).await
    }

    pub async fn put_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = self.url(path);
        let res = self
            .http
            .put(&url)
            .json(body)
            .send()
            .await
            .with_context(|| format!("PUT {url}"))?;
        decode_json(res).await
    }

    /// POST raw bytes (e.g. a `.pexe` archive) and decode the JSON response.
    pub async fn post_bytes<T: DeserializeOwned>(&self, path: &str, bytes: Vec<u8>) -> Result<T> {
        let url = self.url(path);
        let res = self
            .http
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        decode_json(res).await
    }
}

async fn decode_json<T: DeserializeOwned>(res: reqwest::Response) -> Result<T> {
    if !res.status().is_success() {
        return Err(decode_error(res).await);
    }
    Ok(res.json::<T>().await?)
}

async fn decode_error(res: reqwest::Response) -> anyhow::Error {
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    if let Ok(body) = serde_json::from_str::<serde_json::Value>(&text)
        && let Some(msg) = body.get("error").and_then(|v| v.as_str())
    {
        return anyhow!("dobjd error ({status}): {msg}");
    }
    anyhow!("dobjd error ({status}): {text}")
}
