//! Signaling + relay WebSocket endpoint.

use crate::{
    db::Db,
    hub::{encode, Conn, Ended, Hub},
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, State,
    },
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use remotex_common::{
    signaling::{ClientToServer, ServerToClient, MAX_CANDIDATES, MAX_NAME_LEN},
    wire::{self, MAX_FRAME},
};
use remotex_security::ids::{generate_device_id, generate_device_secret};
use sha2::{Digest, Sha256};
use std::{net::IpAddr, net::SocketAddr, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::{
    sync::{mpsc, Notify},
    time::{sleep, timeout, Instant},
};
use tracing::{info, warn};

#[derive(Clone)]
pub struct AppState {
    pub hub: Arc<Hub>,
    pub db: Db,
}

pub async fn ws_handler(
    State(st): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.max_message_size(MAX_FRAME + 4096)
        .max_frame_size(MAX_FRAME + 4096)
        .on_upgrade(move |socket| connection(st, socket, addr.ip().to_canonical()))
}

fn hash_secret(pepper: &str, secret: &[u8]) -> Vec<u8> {
    Sha256::new()
        .chain_update(pepper.as_bytes())
        .chain_update(b":device:")
        .chain_update(secret)
        .finalize()
        .to_vec()
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME_LEN && !name.chars().any(char::is_control)
}

async fn next_binary(
    stream: &mut futures_util::stream::SplitStream<WebSocket>,
    idle: Duration,
) -> Option<Vec<u8>> {
    loop {
        match timeout(idle, stream.next()).await.ok()?? {
            Ok(Message::Binary(b)) => return Some(b.to_vec()),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => continue, // WebSocket-level ping/pong/text are ignored
        }
    }
}

async fn finish(st: &AppState, ended: Vec<Ended>) {
    for e in ended {
        e.notify();
        st.db
            .session_end(e.session.to_hex(), e.mode.as_str(), e.bytes)
            .await;
        info!(target: "connection", session = %e.session.to_hex(), viewer = %e.viewer, host = %e.host, mode = e.mode.as_str(), relay_bytes = e.bytes, "session ended");
    }
}

async fn connection(st: AppState, socket: WebSocket, ip: IpAddr) {
    let (mut sink, mut stream) = socket.split();
    // Shallow on purpose: a deep queue only adds seconds of stale video before an input frame.
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(48);
    let writer = tokio::spawn(async move {
        while let Some(bytes) = rx.recv().await {
            if sink.send(Message::Binary(bytes.into())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    let reply = |m: ServerToClient| {
        let tx = tx.clone();
        async move {
            let _ = tx.send(encode(&m)).await;
        }
    };

    if st.hub.ip_banned_for_auth(ip) {
        info!(target: "security", %ip, "rejected connection from banned address");
        reply(ServerToClient::AuthFailed {
            reason: "temporarily blocked".into(),
        })
        .await;
        drop(tx);
        let _ = writer.await;
        return;
    }

    // ---- authentication phase ------------------------------------------------------------------
    let cfg = st.hub.cfg.clone();
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut device_id: Option<String> = None;
    let mut auth: Option<(String, String, Vec<String>, bool)> = None;
    let mut version = String::new();

    while auth.is_none() {
        let left = deadline.saturating_duration_since(Instant::now());
        let Some(raw) = next_binary(&mut stream, left).await else {
            break;
        };
        let Ok(msg) = wire::decode::<ClientToServer>(&raw) else {
            st.hub.record_auth_failure(ip);
            break;
        };
        match msg {
            ClientToServer::Register { client_version } => {
                if !st.hub.allow_register(ip) {
                    info!(target: "security", %ip, "registration rate limit hit");
                    reply(ServerToClient::AuthFailed {
                        reason: "rate limited".into(),
                    })
                    .await;
                    break;
                }
                let mut registered = false;
                for _ in 0..20 {
                    let id = generate_device_id();
                    let secret = generate_device_secret();
                    match st
                        .db
                        .insert_device(id.as_str().to_string(), hash_secret(&cfg.server_secret, &secret))
                        .await
                    {
                        Ok(true) => {
                            info!(target: "security", %ip, device = %id, version = %client_version, "device registered");
                            st.db
                                .log_attempt(ip.to_string(), "register", true, Some(id.as_str().to_string()))
                                .await;
                            reply(ServerToClient::Registered {
                                device_id: id.as_str().to_string(),
                                secret,
                            })
                            .await;
                            registered = true;
                            break;
                        }
                        Ok(false) => continue,
                        Err(e) => {
                            warn!(error = %e, "database error during registration");
                            break;
                        }
                    }
                }
                if !registered {
                    break;
                }
            }
            ClientToServer::Auth {
                device_id: id,
                secret,
                client_version,
                device_name,
                candidates,
                incoming_enabled,
            } => {
                let hash = st.db.device_hash(id.clone()).await.ok().flatten();
                // Compare against a dummy hash when the id is unknown to keep timing uniform.
                let expected = hash.clone().unwrap_or_else(|| vec![0u8; 32]);
                let ok =
                    hash.is_some() && bool::from(expected.ct_eq(&hash_secret(&cfg.server_secret, &secret)));
                let input_ok = valid_name(&device_name)
                    && client_version.len() <= 32
                    && candidates.len() <= MAX_CANDIDATES
                    && candidates.iter().all(|c| c.parse::<SocketAddr>().is_ok());
                if !ok || !input_ok {
                    let banned = st.hub.record_auth_failure(ip);
                    warn!(target: "security", %ip, device = %id, banned, "device authentication failed");
                    st.db.log_attempt(ip.to_string(), "auth", false, Some(id)).await;
                    reply(ServerToClient::AuthFailed {
                        reason: "invalid credentials".into(),
                    })
                    .await;
                    break;
                }
                st.db
                    .log_attempt(ip.to_string(), "auth", true, Some(id.clone()))
                    .await;
                version = client_version;
                device_id = Some(id.clone());
                auth = Some((id, device_name, candidates, incoming_enabled));
            }
            _ => {
                st.hub.record_auth_failure(ip);
                break;
            }
        }
    }

    let Some((id, name, candidates, incoming_enabled)) = auth else {
        drop(tx);
        let _ = timeout(Duration::from_secs(2), writer).await;
        return;
    };
    let _ = device_id;

    let conn_id = st.hub.next_conn_id();
    let kick = Arc::new(Notify::new());
    st.hub.attach(
        &id,
        Conn {
            conn_id,
            tx: tx.clone(),
            kick: kick.clone(),
            name,
            ip,
            candidates,
            incoming_enabled,
        },
    );
    let _ = st.db.touch_device(id.clone(), version).await;
    reply(ServerToClient::AuthOk {
        public_ip: ip.to_string(),
    })
    .await;
    info!(target: "connection", device = %id, %ip, "device online");

    // ---- main loop -----------------------------------------------------------------------------
    let mut window_start = Instant::now();
    let mut window_count = 0u32;
    let mut last_presence = Instant::now() - Duration::from_secs(60);

    let mut closing = st.hub.shutdown.subscribe();
    loop {
        let raw = tokio::select! {
            r = next_binary(&mut stream, Duration::from_secs(75)) => r,
            _ = closing.changed() => break,
            _ = kick.notified() => {
                reply(ServerToClient::Error { message: "signed in from another location".into() }).await;
                break;
            }
        };
        let Some(raw) = raw else { break };
        let Ok(msg) = wire::decode::<ClientToServer>(&raw) else {
            st.hub
                .metrics
                .errors
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            warn!(target: "security", device = %id, %ip, "malformed message, closing");
            break;
        };

        if !matches!(msg, ClientToServer::Relay { .. }) {
            if window_start.elapsed() > Duration::from_secs(1) {
                window_start = Instant::now();
                window_count = 0;
            }
            window_count += 1;
            if window_count > 60 {
                warn!(target: "security", device = %id, %ip, "control message flood, closing");
                break;
            }
        }

        match msg {
            ClientToServer::Ping => reply(ServerToClient::Pong).await,
            ClientToServer::SetIncomingEnabled(v) => st.hub.update_incoming(&id, conn_id, v),
            ClientToServer::QueryPresence { ids } => {
                if ids.len() > 64 || last_presence.elapsed() < Duration::from_secs(2) {
                    continue;
                }
                last_presence = Instant::now();
                reply(ServerToClient::Presence(st.hub.presence(&ids))).await;
            }
            ClientToServer::Connect { target_id } => match st.hub.connect(&id, conn_id, &target_id) {
                Ok(s) => {
                    if let Some(old) = s.replaced {
                        finish(&st, vec![old]).await;
                    }
                    st.db
                        .session_start(s.session.to_hex(), id.clone(), target_id.replace(' ', ""))
                        .await;
                    info!(target: "connection", session = %s.session.to_hex(), viewer = %id, host = %target_id, same_network = s.same_network, "session requested");
                    let _ = s
                        .host_tx
                        .send(encode(&ServerToClient::Incoming {
                            session: s.session,
                            from_id: id.clone(),
                            from_name: s.viewer_name,
                            same_network: s.same_network,
                        }))
                        .await;
                    reply(ServerToClient::SessionReady {
                        session: s.session,
                        host_name: s.host_name,
                        candidates: s.host_candidates,
                        same_network: s.same_network,
                    })
                    .await;
                }
                Err(reason) => {
                    info!(target: "security", viewer = %id, %ip, ?reason, "connect refused");
                    reply(ServerToClient::ConnectFailed { target_id, reason }).await;
                }
            },
            ClientToServer::Relay { session, data } => {
                if data.len() > MAX_FRAME {
                    break;
                }
                let Some(route) = st.hub.relay_route(&session, &id, data.len()) else {
                    continue;
                };
                if !route.delay.is_zero() {
                    sleep(route.delay).await;
                }
                let out = encode(&ServerToClient::Relay { session, data });
                if timeout(Duration::from_secs(15), route.tx.send(out))
                    .await
                    .is_err()
                {
                    warn!(session = %session.to_hex(), "relay peer stalled, ending session");
                    finish(&st, st.hub.end_by(&session, &id).into_iter().collect()).await;
                }
            }
            ClientToServer::SessionMode { session, direct } => st.hub.set_mode(&session, &id, direct),
            ClientToServer::EndSession { session, auth_failed } => {
                if auth_failed {
                    if let Some((viewer, banned)) = st.hub.report_password_failure(&session, &id) {
                        warn!(target: "security", host = %id, %viewer, banned, "password failure reported by host");
                        st.db
                            .log_attempt(ip.to_string(), "password", false, Some(viewer))
                            .await;
                    }
                }
                finish(&st, st.hub.end_by(&session, &id).into_iter().collect()).await;
            }
            ClientToServer::Register { .. } | ClientToServer::Auth { .. } => break,
        }
    }

    let ended = st.hub.detach(&id, conn_id);
    finish(&st, ended).await;
    info!(target: "connection", device = %id, "device offline");
    drop(tx);
    let _ = timeout(Duration::from_secs(2), writer).await;
}
