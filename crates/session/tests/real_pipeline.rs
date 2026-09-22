//! Real DXGI capture and hardware/software H.264 encoding, streamed over the real server.
//! Skips itself where no desktop session or encoder is available (for example CI).
#![cfg(windows)]

mod support;

use remotex_common::peer::{AuthKind, Permissions};
use remotex_session::{
    platform,
    testing::test_env,
    types::{Event, ViewerState},
};
use std::time::Duration;
use support::{node_with_env, NodeOpts, TestServer};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_desktop_streams_h264_end_to_end() {
    if remotex_capture::list_monitors()
        .map(|m| m.is_empty())
        .unwrap_or(true)
    {
        eprintln!("no desktop available, skipping");
        return;
    }
    let server = TestServer::start().await;
    // Real screen + real encoder on the host; injected input stays mocked so the test never moves the mouse.
    let mut host_env = test_env();
    let real = platform::env();
    host_env.env.screen = real.screen.clone();
    host_env.env.encoders = real.encoders.clone();
    let host = node_with_env(&server, "Real Host", NodeOpts::default(), host_env).await;
    let viewer = node_with_env(&server, "Viewer", NodeOpts::default(), test_env()).await;

    let hm = host.rec.mark();
    viewer
        .engine
        .connect(&host.id, &host.password, AuthKind::Temporary, Permissions::ALL)
        .unwrap();
    let req = host
        .rec
        .wait_from(hm, 10, |e| {
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
        .wait(15, |e| {
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

    // An idle desktop legitimately produces very few frames (unchanged frames are not re-encoded), so the
    // stream is judged on delivery, not on a frame rate: a keyframe must arrive promptly, and the encoder
    // must keep producing whatever the desktop changes into. Nothing here forces identical frames.
    let end = tokio::time::Instant::now() + Duration::from_secs(20);
    while viewer.rec.frames() < 1 {
        assert!(
            tokio::time::Instant::now() < end,
            "only {} frames arrived",
            viewer.rec.frames()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    tokio::time::sleep(Duration::from_millis(1500)).await;
    let log = viewer.rec.video_log.lock().unwrap().clone();
    let (key, w, h, data) = &log[0];
    assert!(*key, "stream must open with a keyframe");
    let nals = remotex_codec::nal_types(data, false);
    assert!(
        nals.contains(&7) && nals.contains(&8) && nals.contains(&5),
        "keyframe carries SPS, PPS and IDR: {nals:?}"
    );
    assert!(*w >= 320 && *h >= 180 && w % 2 == 0 && h % 2 == 0);
    let bytes: usize = log.iter().map(|f| f.3.len()).sum();
    eprintln!(
        "received {} frames, {}x{}, {} KiB total; first frame {} bytes",
        log.len(),
        w,
        h,
        bytes / 1024,
        data.len()
    );

    // Real host telemetry reaches the viewer.
    let end = tokio::time::Instant::now() + Duration::from_secs(10);
    let stats = loop {
        if let Some(st) = viewer
            .rec
            .last_stats
            .lock()
            .unwrap()
            .clone()
            .filter(|s| !s.host.encoder.is_empty())
        {
            break st;
        }
        assert!(tokio::time::Instant::now() < end, "no host telemetry received");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    eprintln!(
        "encoder='{}' fps={:.1} bitrate={} kbps capture={:.2} ms encode={:.2} ms rtt={:.1} ms",
        stats.host.encoder,
        stats.host.fps,
        stats.host.bitrate_kbps,
        stats.host.capture_ms,
        stats.host.encode_ms,
        stats.rtt_ms
    );

    // Persist a sample for the WebCodecs decode check (frames as hex, one per line).
    let dir = std::env::var("REMOTEX_SAMPLE_DIR").unwrap_or_else(|_| "target".into());
    let lines: Vec<String> = log
        .iter()
        .map(|(k, w, h, d)| {
            format!(
                "{} {} {} {}",
                u8::from(*k),
                w,
                h,
                d.iter().map(|b| format!("{b:02x}")).collect::<String>()
            )
        })
        .collect();
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(format!("{dir}/sample-stream.txt"), lines.join("\n")).unwrap();
}
