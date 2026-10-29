use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::{StatusCode, Url, header};
use rusty_s3::{Bucket, Credentials, S3Action as _, UrlStyle};

use crate::board::Leaderboard;

const SIGNED_URL_LIFETIME: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long readers of the public copy may cache it.
const CACHE_CONTROL: &str = "public, max-age=60";

/// An S3-compatible service, such as Cloudflare R2, and credentials for it.
pub struct S3Config {
    pub endpoint: Url,
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

/// A leaderboard object in a bucket.
pub struct Store {
    bucket: Bucket,
    credentials: Credentials,
    object: String,
    client: reqwest::Client,
}

impl Store {
    pub fn new(s3: &S3Config, bucket: String, object: String) -> Result<Self> {
        Ok(Self {
            bucket: Bucket::new(
                s3.endpoint.clone(),
                UrlStyle::Path,
                bucket,
                s3.region.clone(),
            )
            .context("Invalid S3 endpoint")?,
            credentials: Credentials::new(s3.access_key_id.clone(), s3.secret_access_key.clone()),
            object,
            client: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()?,
        })
    }

    /// The stored leaderboard, or an empty one if there is none.
    pub async fn load(&self) -> Result<Leaderboard> {
        let url = self
            .bucket
            .get_object(Some(&self.credentials), &self.object)
            .sign(SIGNED_URL_LIFETIME);
        let response = self.client.get(url).send().await?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(Leaderboard::default()),
            status if status.is_success() => serde_json::from_slice(&response.bytes().await?)
                .with_context(|| format!("{} is not a leaderboard", self.object)),
            status => bail!(
                "Reading {} failed with {status}: {}",
                self.object,
                response.text().await.unwrap_or_default()
            ),
        }
    }

    pub async fn save(&self, leaderboard: &Leaderboard) -> Result<()> {
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
                "Writing {} failed with {status}: {}",
                self.object,
                response.text().await.unwrap_or_default()
            );
        }
        Ok(())
    }
}

/// An S3 service in memory, holding objects by `bucket/key`.
#[cfg(test)]
pub mod mock {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use axum::{
        Router,
        body::Bytes,
        extract::{Path, RawQuery, State},
        http::{HeaderMap, StatusCode},
        routing::get,
    };

    use super::S3Config;

    #[derive(Default)]
    pub struct Service {
        pub objects: HashMap<String, Vec<u8>>,
        pub fail: bool,
        /// Each write's `bucket/key` and headers, in order.
        pub writes: Vec<(String, HeaderMap)>,
        pub queries: Vec<String>,
    }

    pub type Shared = Arc<Mutex<Service>>;

    async fn get_object(
        State(service): State<Shared>,
        Path((bucket, key)): Path<(String, String)>,
        RawQuery(query): RawQuery,
    ) -> (StatusCode, Vec<u8>) {
        let mut service = service.lock().unwrap();
        service.queries.push(query.unwrap_or_default());
        if service.fail {
            return (StatusCode::INTERNAL_SERVER_ERROR, b"down".to_vec());
        }
        match service.objects.get(&format!("{bucket}/{key}")) {
            Some(object) => (StatusCode::OK, object.clone()),
            None => (StatusCode::NOT_FOUND, b"<Code>NoSuchKey</Code>".to_vec()),
        }
    }

    async fn put_object(
        State(service): State<Shared>,
        Path((bucket, key)): Path<(String, String)>,
        RawQuery(query): RawQuery,
        headers: HeaderMap,
        body: Bytes,
    ) -> StatusCode {
        let mut service = service.lock().unwrap();
        service.queries.push(query.unwrap_or_default());
        if service.fail {
            return StatusCode::FORBIDDEN;
        }
        let object = format!("{bucket}/{key}");
        service.objects.insert(object.clone(), body.to_vec());
        service.writes.push((object, headers));
        StatusCode::OK
    }

    pub async fn start() -> (S3Config, Shared) {
        let service = Shared::default();
        let app = Router::new()
            .route("/{bucket}/{*key}", get(get_object).put(put_object))
            .with_state(service.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let s3 = S3Config {
            endpoint: endpoint.parse().unwrap(),
            region: "auto".into(),
            access_key_id: "key".into(),
            secret_access_key: "secret".into(),
        };
        (s3, service)
    }
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;
    use twilight_model::id::Id;

    use super::*;

    async fn mock_store() -> (Store, mock::Shared) {
        let (s3, service) = mock::start().await;
        let store =
            Store::new(&s3, "bitcraft".into(), "leaderboards/bitcraft.json".into()).unwrap();
        (store, service)
    }

    #[tokio::test]
    async fn starts_empty_then_reads_back_what_it_saved() {
        let (store, service) = mock_store().await;
        assert_eq!(store.load().await.unwrap(), Leaderboard::default());
        let mut leaderboard = Leaderboard::default();
        leaderboard.record(Id::new(7), 31.12, OffsetDateTime::UNIX_EPOCH);
        store.save(&leaderboard).await.unwrap();
        assert_eq!(store.load().await.unwrap(), leaderboard);

        let service = service.lock().unwrap();
        let (object, headers) = &service.writes[0];
        assert_eq!(object, "bitcraft/leaderboards/bitcraft.json");
        assert_eq!(headers["content-type"], "application/json");
        assert_eq!(headers["cache-control"], CACHE_CONTROL);
        for query in &service.queries {
            assert!(query.contains("X-Amz-Signature="), "{query}");
        }
        assert!(service.queries[1].contains("cache-control%3Bcontent-type%3Bhost"));
    }

    #[tokio::test]
    async fn unreadable_leaderboards_are_errors_not_empty() {
        let (store, service) = mock_store().await;
        service.lock().unwrap().objects.insert(
            "bitcraft/leaderboards/bitcraft.json".into(),
            b"<html>".to_vec(),
        );
        assert!(store.load().await.is_err());
        service.lock().unwrap().fail = true;
        assert!(store.load().await.is_err());
        assert!(store.save(&Leaderboard::default()).await.is_err());
    }
}
