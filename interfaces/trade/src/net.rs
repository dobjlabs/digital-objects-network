//! One iroh connection, one bidirectional stream, one JSON message
//! per line. The messages are self-authenticating (see `protocol`),
//! so this layer adds nothing but transport.

use anyhow::{Context as _, Result, anyhow};
use iroh::{
    Endpoint, EndpointAddr,
    endpoint::{Connection, RecvStream, SendStream, presets},
};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::protocol::WireMsg;

pub const ALPN: &[u8] = b"zk-craft/trade/0";

pub struct Channel {
    send: SendStream,
    recv: BufReader<RecvStream>,
}

impl Channel {
    pub async fn send(&mut self, msg: &WireMsg) -> Result<()> {
        let mut line = serde_json::to_vec(msg)?;
        line.push(b'\n');
        self.send
            .write_all(&line)
            .await
            .map_err(|err| anyhow!("send failed: {err}"))?;
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<WireMsg> {
        let mut line = String::new();
        let n = self
            .recv
            .read_line(&mut line)
            .await
            .map_err(|err| anyhow!("receive failed: {err}"))?;
        if n == 0 {
            anyhow::bail!("counterparty closed the connection");
        }
        let msg: WireMsg = serde_json::from_str(&line)
            .map_err(|err| anyhow!("unreadable message from counterparty: {err}"))?;
        if let WireMsg::Abort { reason } = &msg {
            anyhow::bail!("counterparty aborted: {reason}");
        }
        Ok(msg)
    }
}

impl Channel {
    /// Signal the end of this side's data. Queued messages still
    /// deliver reliably as long as the connection stays open, so the
    /// sender of the final message finishes and then waits for the
    /// peer to close.
    pub fn finish(&mut self) {
        let _ = self.send.finish();
    }
}

/// Bind the initiator's endpoint, come online, and return it with the
/// dialable address for the invitation.
pub async fn listen() -> Result<(Endpoint, EndpointAddr)> {
    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .map_err(|err| anyhow!("cannot bind iroh endpoint: {err}"))?;
    endpoint.online().await;
    let addr = endpoint.addr();
    Ok((endpoint, addr))
}

/// Wait for the accepter to dial in and open the message stream.
pub async fn accept_peer(endpoint: &Endpoint) -> Result<(Connection, Channel)> {
    let incoming = endpoint
        .accept()
        .await
        .ok_or_else(|| anyhow!("endpoint closed while waiting for the counterparty"))?;
    let connection = incoming
        .await
        .map_err(|err| anyhow!("incoming connection failed: {err}"))?;
    let (send, recv) = connection
        .accept_bi()
        .await
        .map_err(|err| anyhow!("counterparty opened no stream: {err}"))?;
    let channel = Channel {
        send,
        recv: BufReader::new(recv),
    };
    Ok((connection, channel))
}

/// Dial the initiator from the invitation and open the message stream.
pub async fn connect(addr: EndpointAddr) -> Result<(Endpoint, Connection, Channel)> {
    let endpoint = Endpoint::bind(presets::N0)
        .await
        .map_err(|err| anyhow!("cannot bind iroh endpoint: {err}"))?;
    let connection = endpoint
        .connect(addr, ALPN)
        .await
        .map_err(|err| anyhow!("cannot reach the initiator: {err}"))?;
    let (send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|err| anyhow!("cannot open message stream: {err}"))?;
    // QUIC streams only exist for the peer once data flows; sending
    // happens first in this protocol, so nothing to prime.
    let _ = &mut recv;
    let channel = Channel {
        send,
        recv: BufReader::new(recv),
    };
    Ok((endpoint, connection, channel))
}

/// Best-effort abort notice; the deal dies harmlessly either way.
pub async fn abort(channel: &mut Channel, reason: &str) {
    let _ = channel
        .send(&WireMsg::Abort {
            reason: reason.to_string(),
        })
        .await
        .context("abort notice failed");
}
