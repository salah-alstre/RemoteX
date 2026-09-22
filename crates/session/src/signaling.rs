//! Persistent connection to the server: registration, authentication, presence and relayed frames.
//! Reconnects with exponential backoff and never blocks the rest of the application.

use crate::{tls, types::ServerState};
use futures_util::{SinkExt, StreamExt};
use remotex_common::{
    signaling::{ClientToServer, ServerToClient},
    wire::{self, MAX_FRAME},
    SessionId,
};
use std::{sync::Arc, time::Duration};
use tokio::{
    net::TcpStream,
    sync::{mpsc, watch},
    time::{sleep, timeout},
};
use tokio_tungstenite::{
    client_async_tls_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
    Connector,
};

#[derive(Clone, Debug)]
pub struct Credentials {
    pub id: String,
    pub secret: Vec<u8>,
}

pub trait CredentialStore: Send + Sync {
    fn load(&self) -> Option<Credentials>;
    fn save(&self, creds: &Credentials);
    fn clear(&self);
}

pub struct SignalConfig {
    pub url: String,
    pub pins: Vec<[u8; 32]>,
    pub allow_insecure: bool,
    pub client_version: String,
    pub device_name: String,
}

pub enum SigEvent {
    State(ServerState),
    Identity(String),
    Msg(ServerToClient),
}

#[derive(Clone)]
pub struct Signaling {
    tx: mpsc::Sender<ClientToServer>,
}

impl Signaling {
    pub async fn send(&self, msg: ClientToServer) -> bool {
        self.tx.send(msg).await.is_ok()
    }

    pub fn try_send(&self, msg: ClientToServer) -> bool {
        self.tx.try_send(msg).is_ok()
    }

    pub async fn relay(&self, session: SessionId, data: Vec<u8>) -> bool {
        self.send(ClientToServer::Relay { session, data }).await
    }
}

pub struct SignalInputs {
    pub candidates: watch::Receiver<Vec<String>>,
    pub incoming_enabled: watch::Receiver<bool>,
}

pub fn spawn(
    cfg: SignalConfig,
    store: Arc<dyn CredentialStore>,
    inputs: SignalInputs,
    events: mpsc::UnboundedSender<SigEvent>,
) -> Signaling {
    // Shallow: relayed session frames must not pile up ahead of newer ones.
    let (tx, rx) = mpsc::channel(16);
    tokio::spawn(run(cfg, store, inputs, events, rx));
    Signaling { tx }
}

struct Target {
    host: String,
    port: u16,
    tls: bool,
}

fn parse_url(url: &str) -> Result<Target, String> {
    let (tls, rest) = if let Some(r) = url.strip_prefix("wss://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("ws://") {
        (false, r)
    } else {
        return Err("server URL must start with wss:// or ws://".into());
    };
    let authority = rest.split('/').next().unwrap_or("");
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => (
            h.trim_matches(['[', ']']).to_string(),
            p.parse().map_err(|_| "bad port")?,
        ),
        _ => (authority.to_string(), if tls { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err("server URL has no host".into());
    }
    Ok(Target { host, port, tls })
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

/// `patience` (1..=6) stretches every network wait after failed attempts, so a slow or lossy path that
/// needs more than the base allowance still gets through instead of timing out identically forever.
fn patience_for(failures: u32) -> u32 {
    (1 + failures).min(6)
}

async fn connect(cfg: &SignalConfig, patience: u32) -> Result<Ws, String> {
    let target = parse_url(&cfg.url)?;
    if !target.tls && !cfg.allow_insecure {
        return Err("refusing unencrypted connection to the server".into());
    }
    let tcp = timeout(
        Duration::from_secs(8 * u64::from(patience)),
        TcpStream::connect((target.host.as_str(), target.port)),
    )
    .await
    .map_err(|_| {
        tracing::warn!(target: "network", event = "tcp.failed", reason = "timeout", "tcp.failed");
        "connection timed out".to_string()
    })?
    .map_err(|e| {
        tracing::warn!(target: "network", event = "tcp.failed", reason = %e, "tcp.failed");
        e.to_string()
    })?;
    tracing::info!(target: "network", event = "tcp.connected", "tcp.connected");
    let _ = tcp.set_nodelay(true);
    let connector = target
        .tls
        .then(|| Connector::Rustls(tls::client_config(&cfg.pins)));
    let ws_cfg = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME + 8192))
        .max_frame_size(Some(MAX_FRAME + 8192));
    if target.tls {
        tracing::info!(target: "security", event = "tls.handshake.start", pins = cfg.pins.len(), "tls.handshake.start");
    }
    let (ws, _) = timeout(
        Duration::from_secs(10 * u64::from(patience)),
        client_async_tls_with_config(cfg.url.as_str(), tcp, Some(ws_cfg), connector),
    )
    .await
    .map_err(|_| {
        tracing::warn!(target: "security", event = "tls.handshake.failed", reason = "timeout", "tls.handshake.failed");
        "handshake timed out".to_string()
    })?
    .map_err(|e| {
        tracing::warn!(target: "security", event = "tls.handshake.failed", reason = %e, "tls.handshake.failed");
        e.to_string()
    })?;
    tracing::info!(target: "network", event = "websocket.connected", "websocket.connected");
    Ok(ws)
}

async fn recv_msg(ws: &mut Ws, wait: Duration) -> Option<ServerToClient> {
    loop {
        match timeout(wait, ws.next()).await.ok()?? {
            Ok(Message::Binary(b)) => return wire::decode(&b).ok(),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
}

async fn send_msg(ws: &mut Ws, msg: &ClientToServer) -> bool {
    match wire::encode(msg) {
        Ok(b) => ws.send(Message::Binary(b.into())).await.is_ok(),
        Err(_) => false,
    }
}

enum Handshake {
    Ready(String),
    Rejected(String),
    IdentityInvalid,
    Failed(String),
}

async fn authenticate(
    ws: &mut Ws,
    cfg: &SignalConfig,
    store: &Arc<dyn CredentialStore>,
    inputs: &SignalInputs,
    patience: u32,
) -> Handshake {
    let creds = match store.load() {
        Some(c) => c,
        None => {
            tracing::info!(target: "server_connection", event = "identity.created", "identity.created: no stored identity, registering this device");
            if !send_msg(
                ws,
                &ClientToServer::Register {
                    client_version: cfg.client_version.clone(),
                },
            )
            .await
            {
                tracing::warn!(target: "server_connection", event = "registration.failed", reason = "send", "registration.failed");
                return Handshake::Failed("connection closed".into());
            }
            tracing::info!(target: "server_connection", event = "registration.request.sent", "registration.request.sent");
            match recv_msg(ws, Duration::from_secs(10 * u64::from(patience))).await {
                Some(ServerToClient::Registered { device_id, secret }) => {
                    tracing::info!(target: "server_connection", event = "registration.response.received", outcome = "registered", "registration.response.received");
                    let c = Credentials {
                        id: device_id,
                        secret,
                    };
                    store.save(&c);
                    c
                }
                Some(ServerToClient::AuthFailed { reason }) => {
                    tracing::warn!(target: "server_connection", event = "registration.response.received", outcome = "rejected", %reason, "registration.response.received");
                    tracing::warn!(target: "server_connection", event = "registration.failed", %reason, "registration.failed");
                    return Handshake::Rejected(reason);
                }
                other => {
                    tracing::warn!(target: "server_connection", event = "registration.failed", reason = if other.is_none() { "no response (timeout or closed)" } else { "unexpected response" }, "registration.failed");
                    return Handshake::Failed("registration failed".into());
                }
            }
        }
    };
    let auth = ClientToServer::Auth {
        device_id: creds.id.clone(),
        secret: creds.secret.clone(),
        client_version: cfg.client_version.clone(),
        device_name: cfg.device_name.clone(),
        candidates: inputs.candidates.borrow().clone(),
        incoming_enabled: *inputs.incoming_enabled.borrow(),
    };
    if !send_msg(ws, &auth).await {
        return Handshake::Failed("connection closed".into());
    }
    tracing::info!(target: "server_connection", event = "auth.request.sent", "auth.request.sent");
    match recv_msg(ws, Duration::from_secs(10 * u64::from(patience))).await {
        Some(ServerToClient::AuthOk { public_ip }) => {
            let _ = public_ip;
            tracing::info!(target: "server_connection", event = "registration.success", "registration.success");
            Handshake::Ready(creds.id)
        }
        Some(ServerToClient::AuthFailed { reason }) if reason == "invalid credentials" => {
            tracing::warn!(target: "server_connection", event = "registration.failed", %reason, "registration.failed");
            Handshake::IdentityInvalid
        }
        Some(ServerToClient::AuthFailed { reason }) => {
            tracing::warn!(target: "server_connection", event = "registration.failed", %reason, "registration.failed");
            Handshake::Rejected(reason)
        }
        other => {
            tracing::warn!(target: "server_connection", event = "registration.failed", reason = if other.is_none() { "no response (timeout or closed)" } else { "unexpected response" }, "registration.failed");
            Handshake::Failed("authentication failed".into())
        }
    }
}

/// Retry delays in seconds; the last step is the ceiling.
const BACKOFF_STEPS: [u64; 6] = [1, 2, 4, 8, 15, 30];

pub fn backoff_secs(step: usize) -> u64 {
    BACKOFF_STEPS[step.min(BACKOFF_STEPS.len() - 1)]
}

/// Only a connection that stayed up earns a fast retry; flapping (for example two installs sharing
/// one identity kicking each other) keeps climbing the ladder instead of hammering the service.
fn step_after_session(step: usize, uptime: Duration) -> usize {
    if uptime >= Duration::from_secs(20) {
        0
    } else {
        step
    }
}

/// Tells "this computer is offline" from "the RemoteX service is unreachable" with a cheap TCP
/// connect to well-known public endpoints. Cached so a retry loop never spams it.
#[derive(Default)]
struct InternetProbe {
    last: Option<(tokio::time::Instant, bool)>,
}

impl InternetProbe {
    async fn online(&mut self) -> bool {
        if let Some((at, v)) = self.last {
            if at.elapsed() < Duration::from_secs(5) {
                return v;
            }
        }
        let mut ok = false;
        for addr in ["1.1.1.1:443", "8.8.8.8:443"] {
            if timeout(Duration::from_secs(2), TcpStream::connect(addr))
                .await
                .is_ok_and(|r| r.is_ok())
            {
                ok = true;
                break;
            }
        }
        self.last = Some((tokio::time::Instant::now(), ok));
        ok
    }
}

async fn run(
    cfg: SignalConfig,
    store: Arc<dyn CredentialStore>,
    mut inputs: SignalInputs,
    events: mpsc::UnboundedSender<SigEvent>,
    mut out_rx: mpsc::Receiver<ClientToServer>,
) {
    let emit = |s: ServerState| {
        let _ = events.send(SigEvent::State(s));
    };
    let mut step = 0usize;
    let mut attempt = 0u32;
    let mut failures = 0u32;
    let mut was_online = false;
    let mut reset_used = false;
    let mut probe = InternetProbe::default();
    loop {
        attempt += 1;
        emit(if attempt == 1 {
            ServerState::Connecting
        } else {
            ServerState::Reconnecting { attempt: attempt - 1 }
        });
        tracing::info!(target: "server_connection", attempt, "connecting to the RemoteX service");
        let result = async {
            let mut ws = connect(&cfg, patience_for(failures)).await?;
            match authenticate(&mut ws, &cfg, &store, &inputs, patience_for(failures)).await {
                Handshake::Ready(id) => Ok((ws, id)),
                Handshake::Rejected(r) => Err(format!("rejected:{r}")),
                Handshake::IdentityInvalid => Err("identity".to_string()),
                Handshake::Failed(e) => Err(e),
            }
        }
        .await;

        match result {
            Ok((mut ws, id)) => {
                reset_used = false;
                was_online = true;
                failures = 0;
                tracing::info!(target: "server_connection", "online, device registered");
                let _ = events.send(SigEvent::Identity(id));
                emit(ServerState::Online);
                let connected_at = tokio::time::Instant::now();
                serve(&mut ws, &mut inputs, &events, &mut out_rx).await;
                tracing::warn!(target: "server_connection", uptime_secs = connected_at.elapsed().as_secs(), "connection to the service lost");
                step = step_after_session(step, connected_at.elapsed());
                attempt = 1;
                emit(ServerState::Reconnecting { attempt: 1 });
            }
            Err(e) if e == "identity" && !reset_used => {
                // The service no longer knows this device (for example its database was reset): start over once.
                tracing::warn!(target: "server_connection", "device identity unknown to the service, registering again");
                store.clear();
                reset_used = true;
                attempt = 0;
                continue;
            }
            Err(e) => {
                failures += 1;
                tracing::warn!(target: "network", attempt, error = %e, "could not reach the service");
                if e.starts_with("rejected:") {
                    emit(ServerState::Rejected);
                } else if !was_online || failures >= 2 {
                    emit(if probe.online().await {
                        ServerState::ServiceUnavailable
                    } else {
                        ServerState::NoInternet
                    });
                }
                step += 1;
            }
        }

        // Drop anything queued while offline: stale relay frames must not leak into the next connection.
        while out_rx.try_recv().is_ok() {}
        let wait = backoff_secs(step.saturating_sub(1)) * 1000 + rand_jitter();
        sleep(Duration::from_millis(wait)).await;
    }
}

fn rand_jitter() -> u64 {
    u64::from(remotex_security::ids::random_bytes::<1>()[0]) + 1
}

async fn serve(
    ws: &mut Ws,
    inputs: &mut SignalInputs,
    events: &mpsc::UnboundedSender<SigEvent>,
    out_rx: &mut mpsc::Receiver<ClientToServer>,
) {
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    let mut last_seen = tokio::time::Instant::now();
    loop {
        tokio::select! {
            incoming = ws.next() => match incoming {
                Some(Ok(Message::Binary(b))) => {
                    last_seen = tokio::time::Instant::now();
                    match wire::decode::<ServerToClient>(&b) {
                        Ok(ServerToClient::Pong) => {}
                        Ok(msg) => { let _ = events.send(SigEvent::Msg(msg)); }
                        Err(_) => tracing::warn!(target: "security", "malformed message from the service ignored"),
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => { last_seen = tokio::time::Instant::now(); }
            },
            Some(msg) = out_rx.recv() => {
                if !send_msg(ws, &msg).await { return; }
            }
            _ = ping.tick() => {
                if last_seen.elapsed() > Duration::from_secs(60) { return; }
                if !send_msg(ws, &ClientToServer::Ping).await { return; }
            }
            Ok(()) = inputs.incoming_enabled.changed() => {
                let v = *inputs.incoming_enabled.borrow();
                if !send_msg(ws, &ClientToServer::SetIncomingEnabled(v)).await { return; }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_follows_the_documented_ladder() {
        let ladder: Vec<u64> = (0..8).map(backoff_secs).collect();
        assert_eq!(ladder, vec![1, 2, 4, 8, 15, 30, 30, 30]);
    }

    #[test]
    fn patience_grows_with_failures_and_is_capped() {
        let p: Vec<u32> = (0..8).map(patience_for).collect();
        assert_eq!(p, vec![1, 2, 3, 4, 5, 6, 6, 6]);
    }

    #[test]
    fn backoff_resets_only_after_a_stable_connection() {
        assert_eq!(step_after_session(4, Duration::from_secs(2)), 4);
        assert_eq!(step_after_session(4, Duration::from_secs(60)), 0);
    }

    #[test]
    fn url_parsing() {
        let t = parse_url("wss://45.88.9.191:8443/ws").unwrap();
        assert_eq!((t.host.as_str(), t.port, t.tls), ("45.88.9.191", 8443, true));
        let t = parse_url("wss://api.example.com/ws").unwrap();
        assert_eq!((t.host.as_str(), t.port), ("api.example.com", 443));
        let t = parse_url("ws://[::1]:9000/ws").unwrap();
        assert_eq!((t.host.as_str(), t.port, t.tls), ("::1", 9000, false));
        assert!(parse_url("http://x").is_err());
        assert!(parse_url("wss:///ws").is_err());
    }
}
