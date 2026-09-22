//! Measures the control loop over the *production* relay (real Internet path, real TLS, pinned
//! certificate). Two engines on this machine connect through the official service, so the path is
//! PC -> server -> PC and the network round trip is the real one. Ignored by default (needs the network
//! and creates two device identities on the service, kept between runs in the directory named by
//! `REMOTEX_PROD_IDENTITY_DIR` so they are reused):
//!   cargo test --release -p remotex-session --test latency_prod -- --ignored --nocapture
//!
//! The host uses the real desktop capture and hardware encoder (input stays mocked, so the machine's
//! mouse is never moved); the viewer reads the same telemetry the app shows.

#![cfg(windows)]

mod support;

use remotex_common::peer::{AuthKind, InputEvent, Permissions};
use remotex_session::{
    platform,
    store::DpapiCredentialStore,
    testing::test_env,
    types::{Event, ViewerState},
    viewer::ViewerCmd,
};
use std::{sync::Arc, time::Duration};
use support::{node_with_cfg, NodeCfg, NodeOpts};

const URL: &str = "wss://45.88.9.191:8443/ws";
const PIN: &str = "3525ea50960ad2d5a39a526ead38abb74c5fd25eb6c738cb4f66a934df63cad0";

fn pin() -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&PIN[i * 2..i * 2 + 2], 16).unwrap();
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore]
async fn control_loop_over_the_production_relay() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = std::env::var("REMOTEX_PROD_IDENTITY_DIR").unwrap_or_else(|_| "target/prod-identities".into());
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = |name: &str| NodeCfg {
        pins: vec![pin()],
        allow_insecure: false,
        store: Some(Arc::new(DpapiCredentialStore::new(
            format!("{dir}/{name}.bin").into(),
        ))),
    };

    let mut host_env = test_env();
    let real = platform::env();
    host_env.env.screen = real.screen.clone();
    host_env.env.encoders = real.encoders.clone();
    let host = node_with_cfg(URL, "Bench host", NodeOpts::default(), host_env, cfg("host")).await;
    let viewer = node_with_cfg(
        URL,
        "Bench viewer",
        NodeOpts::default(),
        test_env(),
        cfg("viewer"),
    )
    .await;

    let hm = host.rec.mark();
    let session = viewer
        .engine
        .connect(&host.id, &host.password, AuthKind::Temporary, Permissions::ALL)
        .unwrap();
    let req = host
        .rec
        .wait_from(hm, 20, |e| {
            if let Event::Incoming(r) = e {
                Some(r.request_id)
            } else {
                None
            }
        })
        .await;
    host.engine.respond_incoming(req, Some(Permissions::ALL));
    viewer
        .rec
        .wait(30, |e| {
            matches!(
                e,
                Event::Viewer {
                    state: ViewerState::Connected,
                    ..
                }
            )
            .then_some(())
        })
        .await;

    // 8 seconds of 125 Hz pointer movement plus a click every half second.
    let mut tick = tokio::time::interval(Duration::from_millis(8));
    let start = std::time::Instant::now();
    let mut n: u16 = 0;
    while start.elapsed() < Duration::from_secs(8) {
        tick.tick().await;
        n = n.wrapping_add(7);
        viewer
            .engine
            .viewer_command(session, ViewerCmd::Input(InputEvent::MouseMove { x: n, y: n }));
        if n.is_multiple_of(64) {
            viewer.engine.viewer_command(
                session,
                ViewerCmd::Input(InputEvent::MouseButton {
                    button: remotex_common::peer::MouseButton::Left,
                    down: true,
                }),
            );
            viewer.engine.viewer_command(
                session,
                ViewerCmd::Input(InputEvent::MouseButton {
                    button: remotex_common::peer::MouseButton::Left,
                    down: false,
                }),
            );
        }
    }
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let st = viewer
        .rec
        .last_stats
        .lock()
        .unwrap()
        .clone()
        .expect("viewer stats");
    let l = &st.latency;
    println!(
        "\nproduction relay:\n  network RTT (ping/pong)     {:.1} ms\n  input RTT (send->applied->ack) {:.1} ms\n  input queue here            {:.3} ms\n  input queue on host         {:.3} ms\n  host OS apply time          {:.3} ms\n  moves coalesced/dropped     {}\n  host capture / encode       {:.1} / {:.1} ms ({})\n  video                       {:.0} kbps at {:.0} fps",
        st.rtt_ms,
        l.input_rtt_ms,
        l.input_queue_ms,
        l.host_queue_ms,
        l.host_apply_ms,
        l.stale_moves_dropped,
        st.host.capture_ms,
        st.host.encode_ms,
        st.host.encoder,
        st.recv_kbps as f32,
        st.frames_per_sec,
    );
}
