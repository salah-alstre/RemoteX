//! Input-latency benchmark over an emulated network path. Ignored by default because it takes minutes:
//!   cargo test --release -p remotex-session --test latency -- --ignored --nocapture
//!
//! For each scenario the viewer sends mouse movement at 125 Hz plus periodic clicks while the host streams
//! video (and optionally the viewer uploads a large file). The host's input sink timestamps every event
//! it applies; both sides share one monotonic clock, so the numbers are true one-way input latencies.

mod support;

use remotex_common::peer::{AuthKind, Codec, DisplayInfo, InputEvent, MouseButton, Permissions};
use remotex_session::{
    env::{EncodedUnit, EncoderFactory, EncoderRequest, Frame, InputSink, VideoEncoder},
    testing::test_env,
    types::{Event, ViewerState},
    viewer::ViewerCmd,
};
use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use support::{node_with_env_url, NetEm, NodeOpts, TestServer};

/// Encoder whose output size follows the requested bitrate, like a real one.
struct SizedEncoder {
    kbps: Arc<AtomicU32>,
    fps: u32,
    frames: u32,
}

impl VideoEncoder for SizedEncoder {
    fn encode(&mut self, _f: &Frame, force_key: bool) -> Result<Vec<EncodedUnit>, String> {
        let key = force_key || self.frames == 0;
        self.frames += 1;
        let per_frame = (self.kbps.load(Ordering::Relaxed) as usize * 125) / self.fps.max(1) as usize;
        let size = if key { per_frame * 6 } else { per_frame }.max(64);
        let mut data = if key {
            vec![0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65]
        } else {
            vec![0, 0, 0, 1, 0x41]
        };
        data.resize(size, 0x5a);
        Ok(vec![EncodedUnit { data, keyframe: key }])
    }
    fn set_bitrate(&mut self, kbps: u32) {
        self.kbps.store(kbps, Ordering::Relaxed);
    }
    fn name(&self) -> String {
        "Sized mock".into()
    }
    fn codec(&self) -> Codec {
        Codec::H264
    }
}

struct SizedEncoders;

impl EncoderFactory for SizedEncoders {
    fn create(&self, r: &EncoderRequest) -> Result<Box<dyn VideoEncoder>, String> {
        Ok(Box::new(SizedEncoder {
            kbps: Arc::new(AtomicU32::new(r.bitrate_kbps.max(500))),
            fps: r.fps,
            frames: 0,
        }))
    }
    fn available(&self) -> Vec<(Codec, String, bool)> {
        vec![(Codec::H264, "Sized mock".into(), false)]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Applied {
    Move(u16),
    Down(u16),
    Up(u16),
}

type Timeline = Arc<Mutex<Vec<(Applied, Instant)>>>;

struct TimedInput(Timeline);

impl InputSink for TimedInput {
    fn apply(&mut self, event: &InputEvent, _d: &DisplayInfo) {
        let at = Instant::now();
        let mut g = self.0.lock().unwrap();
        match event {
            InputEvent::MouseMove { x, .. } => g.push((Applied::Move(*x), at)),
            InputEvent::MouseButton { down: true, .. } => g.push((Applied::Down(0), at)),
            InputEvent::MouseButton { down: false, .. } => g.push((Applied::Up(0), at)),
            _ => {}
        }
    }
    fn release_all(&mut self) {}
}

struct Scenario {
    name: &'static str,
    rtt_ms: u64,
    kbps: u64,
    file_mb: usize,
    loss: f64,
    /// Skip the relay: a direct TCP connection between the two peers (same network).
    direct: bool,
}

#[derive(Debug)]
struct Outcome {
    sent_moves: usize,
    applied_moves: usize,
    p50: f64,
    p95: f64,
    max: f64,
    click_p50: f64,
    clicks_ok: bool,
    final_ok: bool,
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p) as usize]
}

async fn run(s: &Scenario) -> Outcome {
    let server = TestServer::start().await;
    let net = NetEm::start_lossy(
        server.port(),
        Duration::from_micros(s.rtt_ms * 250),
        s.kbps,
        s.loss,
    )
    .await;
    let url = if s.direct {
        server.url.clone()
    } else {
        format!("ws://127.0.0.1:{}/ws", net.port)
    };
    let opts = || NodeOpts { direct: s.direct };

    let timeline: Timeline = Arc::default();
    let mut host_env = test_env();
    host_env.env.encoders = Arc::new(SizedEncoders);
    let tl = timeline.clone();
    host_env.env.input = Arc::new(move || Box::new(TimedInput(tl.clone())));
    let host = node_with_env_url(&url, "Host", opts(), host_env).await;
    let viewer = node_with_env_url(&url, "Viewer", opts(), test_env()).await;

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
    // Let video and the adapter settle.
    tokio::time::sleep(Duration::from_secs(3)).await;

    if s.file_mb > 0 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        std::fs::write(&path, vec![7u8; s.file_mb * 1024 * 1024]).unwrap();
        viewer
            .engine
            .viewer_command(session, ViewerCmd::SendFiles(vec![path]));
        std::mem::forget(dir);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let mut sent: Vec<(Applied, Instant)> = Vec::new();
    let start = Instant::now();
    let mut n: u16 = 0;
    let mut next_click = start + Duration::from_millis(400);
    let mut tick = tokio::time::interval(Duration::from_millis(8));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    while start.elapsed() < Duration::from_secs(8) {
        tick.tick().await;
        n = n.wrapping_add(1);
        let now = Instant::now();
        sent.push((Applied::Move(n), now));
        viewer
            .engine
            .viewer_command(session, ViewerCmd::Input(InputEvent::MouseMove { x: n, y: n }));
        if now >= next_click {
            next_click = now + Duration::from_millis(400);
            sent.push((Applied::Down(0), now));
            viewer.engine.viewer_command(
                session,
                ViewerCmd::Input(InputEvent::MouseButton {
                    button: MouseButton::Left,
                    down: true,
                }),
            );
            sent.push((Applied::Up(0), now));
            viewer.engine.viewer_command(
                session,
                ViewerCmd::Input(InputEvent::MouseButton {
                    button: MouseButton::Left,
                    down: false,
                }),
            );
        }
    }
    // The last position must arrive; wait generously (queues may still be draining).
    let last_move = n;
    tokio::time::sleep(Duration::from_millis(s.rtt_ms * 2 + 1500)).await;

    let applied = timeline.lock().unwrap().clone();
    let mut lat_moves = Vec::new();
    let mut lat_clicks = Vec::new();
    let sent_at = |a: Applied| sent.iter().find(|(x, _)| *x == a).map(|(_, t)| *t);
    for (a, at) in &applied {
        if let Applied::Move(id) = a {
            if let Some(t) = sent_at(Applied::Move(*id)) {
                lat_moves.push(at.duration_since(t).as_secs_f64() * 1000.0);
            }
        }
    }
    let sent_clicks: Vec<Instant> = sent
        .iter()
        .filter(|(a, _)| matches!(a, Applied::Down(_)))
        .map(|(_, t)| *t)
        .collect();
    let applied_clicks: Vec<(Applied, Instant)> = applied
        .iter()
        .filter(|(a, _)| !matches!(a, Applied::Move(_)))
        .cloned()
        .collect();
    let mut clicks_ok = true;
    for (i, (a, _)) in applied_clicks.iter().enumerate() {
        let want_down = i % 2 == 0;
        if matches!(a, Applied::Down(_)) != want_down {
            clicks_ok = false;
        }
    }
    clicks_ok &= applied_clicks.len() == sent_clicks.len() * 2;
    for (i, t) in sent_clicks.iter().enumerate() {
        if let Some((_, at)) = applied_clicks.get(i * 2) {
            lat_clicks.push(at.duration_since(*t).as_secs_f64() * 1000.0);
        }
    }
    // Ordering of moves: applied ids must never go backwards.
    let ids: Vec<u16> = applied
        .iter()
        .filter_map(|(a, _)| if let Applied::Move(i) = a { Some(*i) } else { None })
        .collect();
    let monotonic = ids.windows(2).all(|w| w[0] < w[1]);
    let final_ok = ids.last() == Some(&last_move) && monotonic;
    Outcome {
        sent_moves: sent.iter().filter(|(a, _)| matches!(a, Applied::Move(_))).count(),
        applied_moves: ids.len(),
        p50: pct(&mut lat_moves.clone(), 0.5),
        p95: pct(&mut lat_moves.clone(), 0.95),
        max: pct(&mut lat_moves, 1.0),
        click_p50: pct(&mut lat_clicks, 0.5),
        clicks_ok,
        final_ok,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore]
async fn input_latency_matrix() {
    let scenarios = [
        Scenario {
            name: "direct LAN (TCP, same network)",
            rtt_ms: 0,
            kbps: 0,
            file_mb: 0,
            loss: 0.0,
            direct: true,
        },
        Scenario {
            name: "loopback via relay, no limit",
            rtt_ms: 0,
            kbps: 0,
            file_mb: 0,
            loss: 0.0,
            direct: false,
        },
        Scenario {
            name: "50 ms RTT, 20 Mbps",
            rtt_ms: 50,
            kbps: 20_000,
            file_mb: 0,
            loss: 0.0,
            direct: false,
        },
        Scenario {
            name: "100 ms RTT, 10 Mbps",
            rtt_ms: 100,
            kbps: 10_000,
            file_mb: 0,
            loss: 0.0,
            direct: false,
        },
        Scenario {
            name: "150 ms RTT, 8 Mbps",
            rtt_ms: 150,
            kbps: 8_000,
            file_mb: 0,
            loss: 0.0,
            direct: false,
        },
        Scenario {
            name: "50 ms RTT, 10 Mbps + file upload",
            rtt_ms: 50,
            kbps: 10_000,
            file_mb: 40,
            loss: 0.0,
            direct: false,
        },
        Scenario {
            name: "150 ms RTT, 4 Mbps + file upload",
            rtt_ms: 150,
            kbps: 4_000,
            file_mb: 40,
            loss: 0.0,
            direct: false,
        },
        Scenario {
            name: "100 ms RTT, 10 Mbps, 1% loss",
            rtt_ms: 100,
            kbps: 10_000,
            file_mb: 0,
            loss: 0.01,
            direct: false,
        },
        Scenario {
            name: "100 ms RTT, 3% loss",
            rtt_ms: 100,
            kbps: 10_000,
            file_mb: 0,
            loss: 0.03,
            direct: false,
        },
        Scenario {
            name: "60 ms RTT, throttled to 2 Mbps",
            rtt_ms: 60,
            kbps: 2_000,
            file_mb: 0,
            loss: 0.0,
            direct: false,
        },
        Scenario {
            name: "60 ms RTT, 2 Mbps + file upload",
            rtt_ms: 60,
            kbps: 2_000,
            file_mb: 20,
            loss: 0.0,
            direct: false,
        },
    ];
    println!(
        "\n{:<36} {:>7} {:>7} {:>8} {:>8} {:>8} {:>9}  ordered",
        "scenario", "sent", "applied", "p50 ms", "p95 ms", "max ms", "click p50"
    );
    let only = std::env::var("LAT_ONLY").unwrap_or_default();
    for s in scenarios.iter().filter(|s| s.name.contains(only.as_str())) {
        let o = run(s).await;
        println!(
            "{:<36} {:>7} {:>7} {:>8.1} {:>8.1} {:>8.1} {:>9.1}  clicks_ok={} final_ok={}",
            s.name, o.sent_moves, o.applied_moves, o.p50, o.p95, o.max, o.click_p50, o.clicks_ok, o.final_ok
        );
    }
}
