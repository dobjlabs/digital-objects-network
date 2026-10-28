use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[cfg(unix)]
type Stream = tokio::net::UnixStream;
#[cfg(windows)]
type Stream = tokio::net::windows::named_pipe::NamedPipeClient;

const FRAME: u32 = 1;
const CLOSE: u32 = 2;
const PING: u32 = 3;
const PONG: u32 = 4;
const MAX_FRAME_BYTES: usize = 64 * 1024;

pub struct Client<S = Stream> {
    stream: S,
}

#[derive(Debug)]
pub struct Rejected {
    message: String,
    pub retry_after: Duration,
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Discord rejected presence: {}", self.message)
    }
}

impl std::error::Error for Rejected {}

impl Client {
    pub async fn connect(application_id: &str) -> Result<Self> {
        #[cfg(unix)]
        let candidates = socket_paths();
        #[cfg(windows)]
        let candidates =
            (0..10).map(|index| std::path::PathBuf::from(format!(r"\\?\pipe\discord-ipc-{index}")));
        Self::connect_candidates(application_id, candidates).await
    }

    async fn connect_candidates(
        application_id: &str,
        candidates: impl IntoIterator<Item = std::path::PathBuf>,
    ) -> Result<Self> {
        let mut last_error = anyhow::anyhow!("Discord is not running");
        for path in candidates {
            let attempt = async {
                #[cfg(unix)]
                let stream = Stream::connect(path).await?;
                #[cfg(windows)]
                let stream = tokio::net::windows::named_pipe::ClientOptions::new().open(path)?;
                let mut client = Self { stream };
                client.handshake(application_id).await?;
                Ok::<_, anyhow::Error>(client)
            };
            match tokio::time::timeout(Duration::from_secs(2), attempt).await {
                Ok(Ok(client)) => return Ok(client),
                Ok(Err(error)) if error.downcast_ref::<std::io::Error>().is_none() => {
                    last_error = error
                }
                Ok(Err(_)) => {}
                Err(error) => last_error = error.into(),
            }
        }
        Err(last_error)
    }
}

#[cfg(unix)]
fn socket_paths() -> Vec<std::path::PathBuf> {
    let mut directories = Vec::new();
    for name in ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"] {
        if let Some(path) = std::env::var_os(name).filter(|path| !path.is_empty()) {
            let path = std::path::PathBuf::from(path);
            if !directories.contains(&path) {
                directories.push(path);
            }
        }
    }
    directories.push("/tmp".into());
    let mut paths = Vec::new();
    for directory in directories {
        for prefix in ["", "app/com.discordapp.Discord", "snap.discord"] {
            for index in 0..10 {
                paths.push(directory.join(prefix).join(format!("discord-ipc-{index}")));
            }
        }
    }
    paths
}

impl<S: AsyncRead + AsyncWrite + Unpin> Client<S> {
    async fn write(&mut self, opcode: u32, payload: &Value) -> Result<()> {
        let bytes = serde_json::to_vec(payload)?;
        self.stream.write_u32_le(opcode).await?;
        self.stream.write_u32_le(bytes.len().try_into()?).await?;
        self.stream.write_all(&bytes).await?;
        self.stream.flush().await?;
        Ok(())
    }

    async fn read(&mut self) -> Result<(u32, Value)> {
        loop {
            let opcode = self.stream.read_u32_le().await?;
            let length = self.stream.read_u32_le().await? as usize;
            if length > MAX_FRAME_BYTES {
                bail!("Discord IPC frame is too large");
            }
            let mut bytes = vec![0; length];
            self.stream.read_exact(&mut bytes).await?;
            let payload: Value = serde_json::from_slice(&bytes)?;
            match opcode {
                CLOSE => bail!(
                    "Discord closed the IPC connection: code={}, message={}",
                    payload["code"],
                    payload["message"]
                ),
                PING => self.write(PONG, &payload).await?,
                FRAME | PONG => return Ok((opcode, payload)),
                _ => bail!("Unknown Discord IPC opcode: {opcode}"),
            }
        }
    }

    async fn handshake(&mut self, application_id: &str) -> Result<()> {
        self.write(0, &json!({"v": 1, "client_id": application_id}))
            .await?;
        let (opcode, reply) = self.read().await?;
        if opcode != FRAME || reply["evt"] != "READY" {
            bail!("Discord did not accept the application ID");
        }
        Ok(())
    }

    pub async fn set_activity(&mut self, activity: Value) -> Result<()> {
        let nonce = uuid::Uuid::new_v4().to_string();
        self.write(
            FRAME,
            &json!({
                "cmd": "SET_ACTIVITY", "nonce": nonce,
                "args": {"pid": std::process::id(), "activity": activity}
            }),
        )
        .await?;
        loop {
            let (opcode, reply) = self.read().await?;
            if opcode == FRAME && reply["nonce"] == nonce {
                if reply["evt"] == "ERROR" {
                    let seconds = reply["data"]["retry_after"]
                        .as_f64()
                        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                        .unwrap_or(60.0);
                    return Err(Rejected {
                        message: reply["data"]["message"]
                            .as_str()
                            .unwrap_or("RPC error")
                            .to_owned(),
                        retry_after: Duration::try_from_secs_f64(seconds)
                            .unwrap_or(Duration::from_secs(60)),
                    }
                    .into());
                }
                if reply["cmd"] != "SET_ACTIVITY" {
                    bail!("Unexpected Discord IPC response");
                }
                return Ok(());
            }
        }
    }

    pub async fn ping(&mut self) -> Result<()> {
        let payload = json!({"nonce": uuid::Uuid::new_v4().to_string()});
        self.write(PING, &payload).await?;
        loop {
            let (opcode, reply) = self.read().await?;
            if opcode == PONG && reply == payload {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
#[cfg(unix)]
pub(super) fn mock_client(
    activities: tokio::sync::mpsc::UnboundedSender<Value>,
) -> (Client, tokio::task::JoinHandle<()>) {
    let (app, discord) = Stream::pair().unwrap();
    let server = tokio::spawn(async move {
        let mut server = Client { stream: discord };
        while let Ok((_, request)) = server.read().await {
            if server
                .write(
                    FRAME,
                    &json!({
                        "cmd": "SET_ACTIVITY", "nonce": request["nonce"], "data": null
                    }),
                )
                .await
                .is_err()
            {
                break;
            }
            if activities
                .send(request["args"]["activity"].clone())
                .is_err()
            {
                break;
            }
        }
    });
    (Client { stream: app }, server)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn discovery_skips_a_stalled_handshake_and_uses_the_next_socket() {
        let dir = tempfile::tempdir().unwrap();
        let stalled = dir.path().join("stalled");
        let healthy = dir.path().join("healthy");
        let first = tokio::net::UnixListener::bind(&stalled).unwrap();
        let second = tokio::net::UnixListener::bind(&healthy).unwrap();
        let stalled_server = tokio::spawn(async move {
            let (_stream, _) = first.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let server = tokio::spawn(async move {
            let (stream, _) = second.accept().await.unwrap();
            let mut server = Client { stream };
            assert_eq!(server.stream.read_u32_le().await.unwrap(), 0);
            let length = server.stream.read_u32_le().await.unwrap();
            let mut bytes = vec![0; length as usize];
            server.stream.read_exact(&mut bytes).await.unwrap();
            server.write(FRAME, &json!({"evt":"READY"})).await.unwrap();
        });
        let client = Client::connect_candidates("123", [stalled, healthy])
            .await
            .unwrap();
        drop(client);
        server.await.unwrap();
        stalled_server.abort();
    }

    #[tokio::test]
    async fn close_errors_preserve_the_discord_diagnostic() {
        let (app, discord) = tokio::io::duplex(4096);
        let mut server = Client { stream: discord };
        server
            .write(CLOSE, &json!({"code":4000,"message":"Invalid client ID"}))
            .await
            .unwrap();
        let error = Client { stream: app }.read().await.unwrap_err().to_string();
        assert!(error.contains("4000"));
        assert!(error.contains("Invalid client ID"));
    }

    #[tokio::test]
    async fn handshake_activity_ping_and_clear_use_the_rpc_protocol() {
        let (app, discord) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            let mut server = Client { stream: discord };
            assert_eq!(server.stream.read_u32_le().await.unwrap(), 0);
            let length = server.stream.read_u32_le().await.unwrap();
            let mut bytes = vec![0; length as usize];
            server.stream.read_exact(&mut bytes).await.unwrap();
            let hello: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(hello, json!({"v": 1, "client_id": "123"}));
            server.write(FRAME, &json!({"evt": "READY"})).await.unwrap();
            for expected in [json!({"details": "Crafting"}), Value::Null] {
                let (_, request) = server.read().await.unwrap();
                assert_eq!(request["args"]["activity"], expected);
                assert_eq!(request["args"]["pid"], std::process::id());
                server.write(PING, &json!({"check": true})).await.unwrap();
                assert_eq!(server.read().await.unwrap(), (PONG, json!({"check": true})));
                server
                    .write(
                        FRAME,
                        &json!({
                            "cmd": "SET_ACTIVITY", "nonce": request["nonce"], "data": null
                        }),
                    )
                    .await
                    .unwrap();
            }
            let opcode = server.stream.read_u32_le().await.unwrap();
            assert_eq!(opcode, PING);
            let length = server.stream.read_u32_le().await.unwrap();
            let mut bytes = vec![0; length as usize];
            server.stream.read_exact(&mut bytes).await.unwrap();
            server
                .write(PONG, &serde_json::from_slice(&bytes).unwrap())
                .await
                .unwrap();
        });
        let mut client = Client { stream: app };
        client.handshake("123").await.unwrap();
        client
            .set_activity(json!({"details": "Crafting"}))
            .await
            .unwrap();
        client.set_activity(Value::Null).await.unwrap();
        client.ping().await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_oversized_frames_and_preserves_retry_delay() {
        let (app, mut discord) = tokio::io::duplex(4096);
        discord.write_u32_le(FRAME).await.unwrap();
        discord
            .write_u32_le(MAX_FRAME_BYTES as u32 + 1)
            .await
            .unwrap();
        assert!(Client { stream: app }.read().await.is_err());

        let (app, discord) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            let mut server = Client { stream: discord };
            let (_, request) = server.read().await.unwrap();
            server
                .write(
                    FRAME,
                    &json!({
                        "evt": "ERROR", "nonce": request["nonce"],
                        "data": {"message": "Rate limited", "retry_after": 45.5}
                    }),
                )
                .await
                .unwrap();
        });
        let error = Client { stream: app }
            .set_activity(Value::Null)
            .await
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<Rejected>().unwrap().retry_after,
            Duration::from_millis(45500)
        );
        server.await.unwrap();
    }
}
