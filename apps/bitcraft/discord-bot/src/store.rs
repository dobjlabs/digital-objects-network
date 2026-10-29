use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::{StatusCode, Url, header};
use rusty_s3::{Bucket, Credentials, S3Action as _, UrlStyle};

use crate::board::Leaderboard;

const SIGNED_URL_LIFETIME: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long readers of the public copy may cache it.
const CACHE_CONTROL: &str = "public, max-age=60";

pub struct StoreConfig {
    pub endpoint: Url,
    pub bucket: String,
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub object: String,
}

/// The leaderboard object in an S3-compatible bucket, such as Cloudflare R2.
pub struct Store {
    bucket: Bucket,
    credentials: Credentials,
    object: String,
    client: reqwest::Client,
}

impl Store {
    pub fn new(config: StoreConfig) -> Result<Self> {
        Ok(Self {
            bucket: Bucket::new(
                config.endpoint,
                UrlStyle::Path,
                config.bucket,
                config.region,
            )
            .context("Invalid S3 endpoint")?,
            credentials: Credentials::new(config.access_key_id, config.secret_access_key),
            object: config.object,
            client: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()?,
        })
    }

    /// The published leaderboard, or an empty one if none has been published.
    pub async fn load(&self) -> Result<Leaderboard> {
        let url = self
            .bucket
            .get_object(Some(&self.credentials), &self.object)
            .sign(SIGNED_URL_LIFETIME);
        let response = self.client.get(url).send().await?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(Leaderboard::default()),
            status if status.is_success() => serde_json::from_slice(&response.bytes().await?)
                .with_context(|| format!("The published {} is not a leaderboard", self.object)),
            status => bail!(
                "Reading {} failed with {status}: {}",
                self.object,
                response.text().await.unwrap_or_default()
            ),
        }
    }

    pub async fn publish(&self, leaderboard: &Leaderboard) -> Result<()> {
        let mut action = self
            .bucket
            .put_object(Some(&self.credentials), &self.object);
        action
            .headers_mut()
            .insert("content-type", "application/json");
        action.headers_mut().insert("cache-control", CACHE_CONTROL);
        let url = action.sign(SIGNED_URL_LIFETIME);
        let response = self
            .client
            .put(url)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CACHE_CONTROL, CACHE_CONTROL)
            .body(serde_json::to_vec_pretty(leaderboard)?)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            bail!(
                "Publishing {} failed with {status}: {}",
                self.object,
                response.text().await.unwrap_or_default()
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        Router,
        body::Bytes,
        extract::{Path, RawQuery, State},
        http::{HeaderMap, StatusCode},
        routing::get,
    };
    use time::OffsetDateTime;
    use twilight_model::id::Id;

    use super::*;

    #[derive(Default)]
    struct Bucket {
        object: Option<Vec<u8>>,
        fail: bool,
        put_headers: Option<HeaderMap>,
        queries: Vec<String>,
    }

    type Shared = Arc<Mutex<Bucket>>;

    async fn get_object(
        State(bucket): State<Shared>,
        Path(path): Path<String>,
        RawQuery(query): RawQuery,
    ) -> (StatusCode, Vec<u8>) {
        let mut bucket = bucket.lock().unwrap();
        assert_eq!(path, "leaderboards/bitcraft.json");
        bucket.queries.push(query.unwrap_or_default());
        match (&bucket.object, bucket.fail) {
            (_, true) => (StatusCode::INTERNAL_SERVER_ERROR, b"down".to_vec()),
            (None, false) => (StatusCode::NOT_FOUND, b"<Code>NoSuchKey</Code>".to_vec()),
            (Some(object), false) => (StatusCode::OK, object.clone()),
        }
    }

    async fn put_object(
        State(bucket): State<Shared>,
        Path(path): Path<String>,
        RawQuery(query): RawQuery,
        headers: HeaderMap,
        body: Bytes,
    ) -> StatusCode {
        let mut bucket = bucket.lock().unwrap();
        assert_eq!(path, "leaderboards/bitcraft.json");
        bucket.queries.push(query.unwrap_or_default());
        if bucket.fail {
            return StatusCode::FORBIDDEN;
        }
        bucket.object = Some(body.to_vec());
        bucket.put_headers = Some(headers);
        StatusCode::OK
    }

    async fn mock_bucket() -> (Store, Shared) {
        let bucket = Shared::default();
        let app = Router::new()
            .route("/bitcraft/{*path}", get(get_object).put(put_object))
            .with_state(bucket.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = Store::new(StoreConfig {
            endpoint: endpoint.parse().unwrap(),
            bucket: "bitcraft".into(),
            region: "auto".into(),
            access_key_id: "key".into(),
            secret_access_key: "secret".into(),
            object: "leaderboards/bitcraft.json".into(),
        })
        .unwrap();
        (store, bucket)
    }

    #[tokio::test]
    async fn starts_empty_then_reads_back_what_it_published() {
        let (store, bucket) = mock_bucket().await;
        assert_eq!(store.load().await.unwrap(), Leaderboard::default());
        let mut leaderboard = Leaderboard::default();
        leaderboard.record(Id::new(7), 31.12, OffsetDateTime::UNIX_EPOCH);
        store.publish(&leaderboard).await.unwrap();
        assert_eq!(store.load().await.unwrap(), leaderboard);

        let bucket = bucket.lock().unwrap();
        let headers = bucket.put_headers.as_ref().unwrap();
        assert_eq!(headers["content-type"], "application/json");
        assert_eq!(headers["cache-control"], CACHE_CONTROL);
        for query in &bucket.queries {
            assert!(query.contains("X-Amz-Signature="), "{query}");
        }
        assert!(bucket.queries[1].contains("cache-control%3Bcontent-type%3Bhost"));
    }

    #[tokio::test]
    async fn unreadable_leaderboards_are_errors_not_empty() {
        let (store, bucket) = mock_bucket().await;
        bucket.lock().unwrap().object = Some(b"<html>".to_vec());
        assert!(store.load().await.is_err());
        bucket.lock().unwrap().fail = true;
        assert!(store.load().await.is_err());
        assert!(store.publish(&Leaderboard::default()).await.is_err());
    }
}
