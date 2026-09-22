//! Protocol-level tests against a real in-process server, including hostile clients.

use futures_util::{SinkExt, StreamExt};
use remotex_common::{
    signaling::{ClientToServer, ConnectFailure, ServerToClient},
    wire, SessionId,
};
use remotex_server::config::Config;
use std::{net::TcpListener, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::Notify,
    time::timeout,
};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Server {
    port: u16,
    stop: Arc<Notify>,
    _dir: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.notify_one();
    }
}

async fn start() -> Server {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = tempfile::tempdir().unwrap();
    let cfg = Arc::new(Config::for_tests(dir.path().join("t.db")));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = Arc::new(Notify::new());
    let s = stop.clone();
    tokio::spawn(async move {
        let _ = remotex_server::serve(cfg, listener, async move { s.notified().await }).await;
    });
    Server {
        port,
        stop,
        _dir: dir,
    }
}

async fn open(s: &Server) -> Ws {
    connect_async(format!("ws://127.0.0.1:{}/ws", s.port))
        .await
        .unwrap()
        .0
}

async fn send(ws: &mut Ws, m: &ClientToServer) {
    ws.send(Message::Binary(wire::encode(m).unwrap().into()))
        .await
        .unwrap();
}

async fn recv(ws: &mut Ws) -> Option<ServerToClient> {
    loop {
        match timeout(Duration::from_secs(3), ws.next()).await.ok()?? {
            Ok(Message::Binary(b)) => return wire::decode(&b).ok(),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
}

async fn register(s: &Server) -> (Ws, String, Vec<u8>) {
    let mut ws = open(s).await;
    send(
        &mut ws,
        &ClientToServer::Register {
            client_version: "test".into(),
        },
    )
    .await;
    let Some(ServerToClient::Registered { device_id, secret }) = recv(&mut ws).await else {
        panic!("registration failed")
    };
    (ws, device_id, secret)
}

fn auth(id: &str, secret: &[u8], incoming: bool) -> ClientToServer {
    ClientToServer::Auth {
        device_id: id.into(),
        secret: secret.to_vec(),
        client_version: "test".into(),
        device_name: "Test".into(),
        candidates: vec![],
        incoming_enabled: incoming,
    }
}

async fn online(s: &Server, incoming: bool) -> (Ws, String) {
    let (mut ws, id, secret) = register(s).await;
    send(&mut ws, &auth(&id, &secret, incoming)).await;
    assert!(matches!(recv(&mut ws).await, Some(ServerToClient::AuthOk { .. })));
    (ws, id)
}

#[tokio::test]
async fn registration_yields_valid_unique_ids() {
    let s = start().await;
    let (_a, id_a, sec_a) = register(&s).await;
    let (_b, id_b, _) = register(&s).await;
    assert_ne!(id_a, id_b);
    assert_eq!(id_a.len(), 9);
    assert_eq!(sec_a.len(), 32);
}

#[tokio::test]
async fn wrong_secret_is_rejected_and_repeated_failures_ban_the_address() {
    let s = start().await;
    let (_ws, id, _secret) = register(&s).await;
    let mut banned = false;
    for _ in 0..12 {
        let mut ws = open(&s).await;
        send(&mut ws, &auth(&id, &[9u8; 32], true)).await;
        match recv(&mut ws).await {
            Some(ServerToClient::AuthFailed { reason }) if reason.contains("blocked") => banned = true,
            Some(ServerToClient::AuthFailed { .. }) | None => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(
        banned,
        "brute-forcing device secrets must trigger a temporary ban"
    );
}

#[tokio::test]
async fn malformed_and_oversized_frames_close_the_connection() {
    let s = start().await;
    let (mut ws, _) = online(&s, true).await;
    ws.send(Message::Binary(vec![0xff; 40].into())).await.unwrap();
    assert!(
        recv(&mut ws).await.is_none(),
        "garbage must terminate the session"
    );

    let mut ws2 = open(&s).await;
    ws2.send(Message::Binary(vec![1, 2, 3].into())).await.unwrap();
    assert!(recv(&mut ws2).await.is_none());

    // Unauthenticated clients cannot use control messages.
    let mut ws3 = open(&s).await;
    send(
        &mut ws3,
        &ClientToServer::Connect {
            target_id: "123456789".into(),
        },
    )
    .await;
    assert!(recv(&mut ws3).await.is_none());
}

#[tokio::test]
async fn relay_only_reaches_session_members() {
    let s = start().await;
    let (mut host, host_id) = online(&s, true).await;
    let (mut viewer, _viewer_id) = online(&s, true).await;
    let (mut mallory, _) = online(&s, true).await;

    send(&mut viewer, &ClientToServer::Connect { target_id: host_id }).await;
    let Some(ServerToClient::Incoming { session, .. }) = recv(&mut host).await else {
        panic!("no incoming")
    };
    let Some(ServerToClient::SessionReady { .. }) = recv(&mut viewer).await else {
        panic!("no ready")
    };

    // A third party (or a guess at the session id) cannot inject frames.
    send(
        &mut mallory,
        &ClientToServer::Relay {
            session,
            data: b"evil".to_vec(),
        },
    )
    .await;
    send(
        &mut mallory,
        &ClientToServer::Relay {
            session: SessionId([7; 16]),
            data: b"evil".to_vec(),
        },
    )
    .await;
    send(
        &mut mallory,
        &ClientToServer::EndSession {
            session,
            auth_failed: true,
        },
    )
    .await;

    // Members can talk in both directions.
    send(
        &mut viewer,
        &ClientToServer::Relay {
            session,
            data: b"hello".to_vec(),
        },
    )
    .await;
    match recv(&mut host).await {
        Some(ServerToClient::Relay { data, .. }) => assert_eq!(data, b"hello"),
        other => panic!("expected relayed frame, got {other:?}"),
    }
    send(
        &mut host,
        &ClientToServer::Relay {
            session,
            data: b"ack".to_vec(),
        },
    )
    .await;
    match recv(&mut viewer).await {
        Some(ServerToClient::Relay { data, .. }) => assert_eq!(data, b"ack"),
        other => panic!("expected relayed frame, got {other:?}"),
    }
}

#[tokio::test]
async fn connect_rules_offline_busy_and_not_accepting() {
    let s = start().await;
    let (mut viewer, _) = online(&s, true).await;
    let (_closed, closed_id) = online(&s, false).await;

    send(
        &mut viewer,
        &ClientToServer::Connect {
            target_id: "100000001".into(),
        },
    )
    .await;
    assert!(matches!(
        recv(&mut viewer).await,
        Some(ServerToClient::ConnectFailed {
            reason: ConnectFailure::Offline,
            ..
        })
    ));
    send(&mut viewer, &ClientToServer::Connect { target_id: closed_id }).await;
    assert!(matches!(
        recv(&mut viewer).await,
        Some(ServerToClient::ConnectFailed {
            reason: ConnectFailure::NotAccepting,
            ..
        })
    ));
    send(
        &mut viewer,
        &ClientToServer::Connect {
            target_id: "not-an-id".into(),
        },
    )
    .await;
    assert!(matches!(
        recv(&mut viewer).await,
        Some(ServerToClient::ConnectFailed {
            reason: ConnectFailure::InvalidId,
            ..
        })
    ));

    let (mut host, host_id) = online(&s, true).await;
    send(
        &mut viewer,
        &ClientToServer::Connect {
            target_id: host_id.clone(),
        },
    )
    .await;
    assert!(matches!(
        recv(&mut host).await,
        Some(ServerToClient::Incoming { .. })
    ));
    let (mut second, _) = online(&s, true).await;
    send(&mut second, &ClientToServer::Connect { target_id: host_id }).await;
    assert!(matches!(
        recv(&mut second).await,
        Some(ServerToClient::ConnectFailed {
            reason: ConnectFailure::Busy,
            ..
        })
    ));
}

#[tokio::test]
async fn id_enumeration_is_throttled() {
    let s = start().await;
    let (mut viewer, _) = online(&s, true).await;
    let mut banned = false;
    for n in 0..25u32 {
        send(
            &mut viewer,
            &ClientToServer::Connect {
                target_id: format!("{}", 100_000_000 + n),
            },
        )
        .await;
        if let Some(ServerToClient::ConnectFailed {
            reason: ConnectFailure::Banned | ConnectFailure::RateLimited,
            ..
        }) = recv(&mut viewer).await
        {
            banned = true;
            break;
        }
    }
    assert!(banned, "scanning for device ids must be rate limited");
}

#[tokio::test]
async fn reported_password_failures_ban_the_viewer() {
    let s = start().await;
    let (mut host, host_id) = online(&s, true).await;
    let (mut viewer, _) = online(&s, true).await;
    for _ in 0..5 {
        send(
            &mut viewer,
            &ClientToServer::Connect {
                target_id: host_id.clone(),
            },
        )
        .await;
        let Some(ServerToClient::Incoming { session, .. }) = recv(&mut host).await else {
            panic!("no incoming")
        };
        let _ = recv(&mut viewer).await;
        send(
            &mut host,
            &ClientToServer::EndSession {
                session,
                auth_failed: true,
            },
        )
        .await;
        // Both participants are told the session ended.
        assert!(matches!(
            recv(&mut viewer).await,
            Some(ServerToClient::SessionEnded { .. })
        ));
        assert!(matches!(
            recv(&mut host).await,
            Some(ServerToClient::SessionEnded { .. })
        ));
    }
    send(&mut viewer, &ClientToServer::Connect { target_id: host_id }).await;
    assert!(matches!(
        recv(&mut viewer).await,
        Some(ServerToClient::ConnectFailed {
            reason: ConnectFailure::Banned,
            ..
        })
    ));
}

async fn http_get(port: u16, path: &str, bearer: Option<&str>) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let auth = bearer
        .map(|b| format!("Authorization: Bearer {b}\r\n"))
        .unwrap_or_default();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n{auth}Connection: close\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    out
}

#[tokio::test]
async fn health_is_public_but_admin_stats_need_the_token() {
    let s = start().await;
    assert!(http_get(s.port, "/health", None).await.contains("\"ok\":true"));
    assert!(http_get(s.port, "/admin/api/stats", None)
        .await
        .starts_with("HTTP/1.1 401"));
    assert!(
        http_get(s.port, "/admin/api/stats", Some("wrong-token-wrong-token"))
            .await
            .starts_with("HTTP/1.1 401")
    );
    let ok = http_get(s.port, "/admin/api/stats", Some(&"t".repeat(24))).await;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(ok.contains("online_devices"));
    // Repeated bad tokens lock the address out even for the right token.
    for _ in 0..6 {
        http_get(s.port, "/admin/api/stats", Some("nope")).await;
    }
    assert!(http_get(s.port, "/admin/api/stats", Some(&"t".repeat(24)))
        .await
        .starts_with("HTTP/1.1 429"));
}
