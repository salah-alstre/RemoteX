//! Certificate verification is never disabled: only a pinned (or publicly trusted) server is accepted.

mod support;

use remotex_session::{
    testing::{test_env, MemoryStore},
    tls::parse_pin,
    types::{EngineConfig, Event, ServerState},
    Engine,
};
use std::{sync::Arc, time::Duration};
use support::TestServer;

/// Returns the device id if the engine gets online, otherwise the states it reported.
async fn try_connect(url: &str, pins: Vec<[u8; 32]>) -> Result<String, Vec<ServerState>> {
    let env = test_env();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = EngineConfig::new(url, "TLS test", dir.path().to_path_buf());
    cfg.cert_pins = pins;
    cfg.direct_enabled = false;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let _engine = Engine::start(cfg, env.env, Arc::new(MemoryStore::default()), tx)
        .await
        .unwrap();
    let mut seen = Vec::new();
    let end = tokio::time::Instant::now() + Duration::from_secs(6);
    while let Ok(Some(ev)) = tokio::time::timeout_at(end, rx.recv()).await {
        match ev {
            Event::Identity { id } => return Ok(id),
            Event::Server(s) => seen.push(s),
            _ => {}
        }
    }
    Err(seen)
}

fn unavailable(states: &[ServerState]) -> bool {
    states
        .iter()
        .any(|s| matches!(s, ServerState::ServiceUnavailable | ServerState::NoInternet))
        && !states.iter().any(|s| matches!(s, ServerState::Online))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pinned_self_signed_certificate_is_accepted() {
    let server = TestServer::start_tls().await;
    let pin = parse_pin(server.fingerprint.as_deref().unwrap()).unwrap();
    let id = try_connect(&server.url, vec![pin])
        .await
        .expect("pinned connection must succeed");
    assert_eq!(id.len(), 9);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn either_current_or_backup_pin_is_trusted() {
    let server = TestServer::start_tls().await;
    let real = parse_pin(server.fingerprint.as_deref().unwrap()).unwrap();
    // Server presents the certificate that is only listed as the backup (mid-rotation).
    try_connect(&server.url, vec![[1u8; 32], real])
        .await
        .expect("backup pin must be accepted");
    try_connect(&server.url, vec![real, [2u8; 32]])
        .await
        .expect("current pin must be accepted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn certificate_matching_no_configured_pin_is_rejected() {
    let server = TestServer::start_tls().await;
    let err = try_connect(&server.url, vec![[7u8; 32], [8u8; 32]])
        .await
        .expect_err("an unknown certificate must never connect");
    assert!(unavailable(&err), "{err:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn self_signed_without_pins_falls_back_to_web_pki_and_is_rejected() {
    let server = TestServer::start_tls().await;
    let err = try_connect(&server.url, Vec::new())
        .await
        .expect_err("unknown CA must be refused");
    assert!(unavailable(&err), "{err:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plain_websocket_is_refused_unless_explicitly_allowed() {
    let server = TestServer::start().await;
    let err = try_connect(&server.url, Vec::new())
        .await
        .expect_err("ws:// must be refused by default");
    assert!(unavailable(&err), "{err:?}");
}
