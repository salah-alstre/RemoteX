//! A `Link` is an ordered, reliable stream of opaque frames to the peer, carried either directly
//! over TCP (LAN) or through the server relay. Everything above it is end-to-end encrypted.

use remotex_common::wire::MAX_FRAME;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
    time::timeout,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkKind {
    Direct,
    Relay,
}

/// Frames allowed to wait between the session scheduler and the transport. Deliberately tiny: the
/// scheduler chooses what to send only when there is room, so everything queued here is already
/// committed and delays whatever is decided later.
pub const OUT_DEPTH: usize = 4;

pub struct Link {
    pub kind: LinkKind,
    pub tx: mpsc::Sender<Vec<u8>>,
    pub rx: mpsc::Receiver<Vec<u8>>,
    pub peer: String,
    /// A frame already read from `rx` (used when the first frame decided which link to use).
    pub pending: Option<Vec<u8>>,
}

impl Link {
    pub fn new(kind: LinkKind, tx: mpsc::Sender<Vec<u8>>, rx: mpsc::Receiver<Vec<u8>>, peer: String) -> Self {
        Self {
            kind,
            tx,
            rx,
            peer,
            pending: None,
        }
    }

    /// Waits (briefly) until everything queued on this link has been handed to the transport, so a
    /// final message is not overtaken by the session-teardown signal.
    pub async fn drain(&self) {
        for _ in 0..100 {
            if self.tx.capacity() == self.tx.max_capacity() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    pub async fn recv(&mut self) -> Option<Vec<u8>> {
        match self.pending.take() {
            Some(f) => Some(f),
            None => self.rx.recv().await,
        }
    }
}

/// Wraps a TCP stream with `u32` length-prefixed framing.
pub fn from_tcp(stream: TcpStream, kind: LinkKind) -> Link {
    let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    let _ = stream.set_nodelay(true);
    let (mut rd, mut wr) = stream.into_split();
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(OUT_DEPTH);
    let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>(64);

    tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            // One write per frame: with TCP_NODELAY a separate 4-byte length write would be its own packet.
            let mut buf = Vec::with_capacity(4 + frame.len());
            buf.extend_from_slice(&(frame.len() as u32).to_be_bytes());
            buf.extend_from_slice(&frame);
            if wr.write_all(&buf).await.is_err() {
                break;
            }
        }
        let _ = wr.shutdown().await;
    });
    tokio::spawn(async move {
        loop {
            let mut len = [0u8; 4];
            if rd.read_exact(&mut len).await.is_err() {
                break;
            }
            let n = u32::from_be_bytes(len) as usize;
            if n == 0 || n > MAX_FRAME + 64 {
                break;
            }
            let mut buf = vec![0u8; n];
            if rd.read_exact(&mut buf).await.is_err() || in_tx.send(buf).await.is_err() {
                break;
            }
        }
    });
    Link::new(kind, out_tx, in_rx, peer)
}

/// Tries each candidate address in parallel; the first that connects and echoes the session preamble wins.
pub async fn connect_direct(candidates: &[String], preamble: [u8; 16], max: Duration) -> Option<Link> {
    let mut set = tokio::task::JoinSet::new();
    for c in candidates.iter().take(8) {
        let addr = c.clone();
        set.spawn(async move {
            let mut s = timeout(max, TcpStream::connect(&addr)).await.ok()?.ok()?;
            s.write_all(&preamble).await.ok()?;
            let mut ack = [0u8; 1];
            timeout(max, s.read_exact(&mut ack)).await.ok()?.ok()?;
            (ack[0] == 1).then_some(s)
        });
    }
    while let Some(res) = set.join_next().await {
        if let Ok(Some(stream)) = res {
            set.abort_all();
            return Some(from_tcp(stream, LinkKind::Direct));
        }
    }
    None
}

/// Two links wired back to back, for tests and in-process pairing.
pub fn memory_pair(kind: LinkKind) -> (Link, Link) {
    let (a_tx, b_rx) = mpsc::channel(OUT_DEPTH);
    let (b_tx, a_rx) = mpsc::channel(OUT_DEPTH);
    (
        Link::new(kind, a_tx, a_rx, "memory".into()),
        Link::new(kind, b_tx, b_rx, "memory".into()),
    )
}
