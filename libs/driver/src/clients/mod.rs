use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::de::DeserializeOwned;

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

/// Whether a request failed without any response, by connection failure or timeout.
pub(crate) fn is_unreachable(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|err| err.is_connect() || err.is_timeout())
    })
}

fn send_json_request<T: DeserializeOwned>(
    request: reqwest::blocking::RequestBuilder,
    endpoint: &str,
    decode_context: &str,
) -> Result<T> {
    let response = request
        .send()
        .with_context(|| format!("failed to request {decode_context} at {endpoint}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let detail = response
            .text()
            .map(|body| body.trim().chars().take(2048).collect::<String>())
            .unwrap_or_else(|error| format!("could not read error response: {error}"));
        return Err(anyhow!(
            "{decode_context} at {endpoint} failed with {} {}: {}",
            status.as_u16(),
            status,
            detail,
        ));
    }

    response
        .json()
        .with_context(|| format!("failed to decode {decode_context}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    fn request(status: &str, body: &str) -> Result<serde_json::Value> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            for line in BufReader::new(&mut stream).lines() {
                if line.unwrap().is_empty() {
                    break;
                }
            }
            stream.write_all(response.as_bytes()).unwrap();
        });
        let result = send_json_request(
            build_http_client().get(&endpoint),
            &endpoint,
            "test response",
        );
        server.join().unwrap();
        result
    }

    #[test]
    fn json_requests_decode_success_and_describe_response_failures() {
        assert_eq!(
            request("200 OK", r#"{"accepted":true}"#).unwrap()["accepted"],
            true
        );
        let error = request("200 OK", "not JSON").unwrap_err();
        assert!(format!("{error:#}").contains("failed to decode test response"));
        let error = request("503 Service Unavailable", &"x".repeat(3000))
            .unwrap_err()
            .to_string();
        assert!(error.contains("test response"));
        assert!(error.contains("503"));
        assert!(error.ends_with(&"x".repeat(2048)));
        assert!(!error.contains(&"x".repeat(2049)));
    }

    #[test]
    fn only_requests_without_a_response_are_unreachable() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let refused = send_json_request::<serde_json::Value>(
            build_http_client().get(&endpoint),
            &endpoint,
            "test response",
        )
        .unwrap_err();
        assert!(is_unreachable(&refused));
        assert!(!is_unreachable(
            &request("503 Service Unavailable", "down").unwrap_err()
        ));
        assert!(!is_unreachable(&request("200 OK", "not JSON").unwrap_err()));
    }
}
