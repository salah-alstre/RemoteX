//! Viewer side of an authenticated session: forwards input, receives video/cursor/clipboard/files
//! and produces real link telemetry.

use crate::{
    env::PlatformEnv,
    host::now_ms,
    secure::{SecureRx, SecureTx},
    transfer::{FileManager, FilePolicy, Side},
    types::{EndReason, EngineConfig, Event, EventTx, HostStatsView, LatencyView, NetQuality, ViewerStats},
};
use remotex_common::peer::{ClipboardPayload, DisplayInfo, InputEvent, Msg, Permissions, QualitySettings};
use remotex_input::ClipboardSync;
use std::{
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

pub enum ViewerCmd {
    Input(InputEvent),
    SelectDisplay(u32),
    SetQuality(QualitySettings),
    RequestKeyframe,
    SetClipboardSync(bool),
    Chat(String),
    SendFiles(Vec<PathBuf>),
    Download(String),
    ListDir(String),
    FilePause(u64),
    FileResume(u64),
    FileCancel(u64),
    FileRetry(u64),
    Disconnect,
}

#[derive(Clone)]
pub struct ViewerHandle {
    pub cmd: mpsc::UnboundedSender<ViewerCmd>,
    /// Direct path for input events: they go straight to the session's send queue instead of waiting
    /// their turn in the session loop. Empty while the session is (re)connecting.
    pub input: Arc<std::sync::Mutex<Option<SecureTx>>>,
}

pub struct ViewerSession<'a> {
    pub session_no: u64,
    pub env: PlatformEnv,
    pub events: EventTx,
    pub cfg: Arc<EngineConfig>,
    pub tx: SecureTx,
    pub rx: SecureRx,
    pub closed: watch::Receiver<bool>,
    pub cmd_rx: &'a mut mpsc::UnboundedReceiver<ViewerCmd>,
    pub permissions: Permissions,
    pub displays: Vec<DisplayInfo>,
}

pub async fn run(mut s: ViewerSession<'_>) -> EndReason {
    let ViewerSession {
        session_no,
        ref env,
        ref events,
        ref cfg,
        ref tx,
        ..
    } = s;
    let mut perms = s.permissions;
    let files = FileManager::new(
        FilePolicy {
            side: Side::Viewer,
            download_dir: cfg.download_dir.clone(),
            ask_before_receiving: false,
            max_file_size: cfg.max_file_size,
            max_active: cfg.max_transfers,
        },
        session_no,
        tx.clone(),
        events.clone(),
        perms.files,
    );
    let mut clipboard =
        (env.clipboard)(cfg.clipboard_images).map(|b| ClipboardSync::new(b, 16 * 1024 * 1024));
    let mut clipboard_on = true;

    let mut ping = tokio::time::interval(Duration::from_secs(1));
    let mut probe = tokio::time::interval(Duration::from_millis(100));
    let mut last_bulk = 0u64;
    let mut governor = crate::bulk::BulkGovernor::default();
    let mut clip_tick = tokio::time::interval(Duration::from_millis(300));
    let mut last_rx = Instant::now();

    // Telemetry state.
    let (mut rtt, mut jitter, mut prev_rtt) = (0f32, 0f32, None::<f32>);
    let mut host_stats = HostStatsView::default();
    let (mut frames, mut last_bytes, mut last_tick) = (0u32, 0u64, Instant::now());
    let (mut dropped_prev, mut delivered_win) = (0u64, 0u32);

    let reason = loop {
        tokio::select! {
            msg = s.rx.recv() => {
                let Some(msg) = msg else { break EndReason::ConnectionLost };
                last_rx = Instant::now();
                match msg {
                    Msg::Video(frame) => {
                        frames += 1;
                        delivered_win += 1;
                        let _ = events.send(Event::Video { session: session_no, frame });
                    }
                    Msg::Cursor(update) => { let _ = events.send(Event::Cursor { session: session_no, update: update.into() }); }
                    Msg::DisplaysChanged { displays, active } => {
                        s.displays = displays.clone();
                        let _ = events.send(Event::ViewerDisplays { session: session_no, displays, active });
                    }
                    Msg::PermissionsChanged(p) => {
                        perms = p;
                        files.set_allowed(p.files);
                        let _ = events.send(Event::ViewerPermissions { session: session_no, permissions: p });
                    }
                    Msg::Stats(st) => host_stats = st.into(),
                    Msg::Clipboard(p) if perms.clipboard && clipboard_on => {
                        let notify = match &p {
                            ClipboardPayload::Text(t) => Event::Clipboard { session: session_no, text: Some(t.chars().take(200).collect()), image: false },
                            ClipboardPayload::Image(_) => Event::Clipboard { session: session_no, text: None, image: true },
                        };
                        if let Some(c) = clipboard.as_mut() {
                            if c.apply_remote(&p).is_ok() {
                                let _ = events.send(notify);
                            }
                        }
                    }
                    Msg::File(f) => files.on_msg(f).await,
                    Msg::Chat { id, text, ts_ms } => { let _ = events.send(Event::Chat { session: session_no, id, text, ts_ms, mine: false }); }
                    Msg::Ping { t } => { tx.send(Msg::Pong { t }); }
                    Msg::Pong { t } => {
                        let sample = now_ms().saturating_sub(t) as f32;
                        if let Some(p) = prev_rtt { jitter += ((sample - p).abs() - jitter) / 16.0; }
                        prev_rtt = Some(sample);
                        rtt = if rtt == 0.0 { sample } else { rtt * 0.7 + sample * 0.3 };
                        tx.counters.bulk_rate_bps.store(governor.update(sample), Ordering::Relaxed);
                    }
                    Msg::Bye => break EndReason::PeerClosed,
                    Msg::Rejected(_) | Msg::Accepted { .. } => {}
                    _ => {}
                }
            }
            Some(cmd) = s.cmd_rx.recv() => match cmd {
                ViewerCmd::Input(ev) => { tx.send_input(ev); }
                ViewerCmd::SelectDisplay(i) => { tx.send(Msg::SelectDisplay(i)); }
                ViewerCmd::SetQuality(q) => { tx.send(Msg::SetQuality(q)); }
                ViewerCmd::RequestKeyframe => { tx.send(Msg::RequestKeyframe); }
                ViewerCmd::SetClipboardSync(on) => clipboard_on = on,
                ViewerCmd::Chat(text) => {
                    let (id, ts_ms) = (now_ms(), now_ms());
                    tx.send(Msg::Chat { id, text: text.clone(), ts_ms });
                    let _ = events.send(Event::Chat { session: session_no, id, text, ts_ms, mine: true });
                }
                ViewerCmd::SendFiles(paths) => {
                    if let Err(e) = files.send_paths(paths) {
                        let _ = events.send(Event::Security { message: e });
                    }
                }
                ViewerCmd::Download(p) => { if let Err(e) = files.download(p) { let _ = events.send(Event::Security { message: e }); } }
                ViewerCmd::ListDir(p) => files.list_dir(p),
                ViewerCmd::FilePause(id) => files.pause(id),
                ViewerCmd::FileResume(id) => files.resume(id),
                ViewerCmd::FileCancel(id) => files.cancel(id),
                ViewerCmd::FileRetry(id) => { let _ = files.retry(id); }
                ViewerCmd::Disconnect => {
                    tx.send(Msg::Bye);
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    break EndReason::LocalClosed;
                }
            },
            _ = clip_tick.tick(), if perms.clipboard && clipboard_on => {
                if let Some(c) = clipboard.as_mut() {
                    if let Some(p) = c.poll_local() {
                        match p {
                            ClipboardPayload::Image(_) => { tx.send_bulk(Msg::Clipboard(p)).await; }
                            text => { tx.send(Msg::Clipboard(text)); }
                        }
                    }
                }
            }
            _ = probe.tick() => {
                let sent = tx.counters.bulk_bytes_sent.load(Ordering::Relaxed);
                if sent != last_bulk {
                    last_bulk = sent;
                    tx.send(Msg::Ping { t: now_ms() });
                }
            }
            _ = ping.tick() => {
                tx.send(Msg::Ping { t: now_ms() });
                if last_rx.elapsed() > Duration::from_secs(15) { break EndReason::ConnectionLost; }
                let dt = last_tick.elapsed().as_secs_f32().max(0.001);
                let bytes = tx.counters.bytes_received.load(Ordering::Relaxed);
                let dropped_total = host_stats.dropped;
                let dropped_delta = dropped_total.saturating_sub(dropped_prev);
                dropped_prev = dropped_total;
                let total = dropped_delta as f32 + delivered_win as f32;
                let dropped_percent = if total > 0.0 { dropped_delta as f32 / total * 100.0 } else { 0.0 };
                let stats = ViewerStats {
                    rtt_ms: rtt,
                    jitter_ms: jitter,
                    recv_kbps: ((bytes - last_bytes) as f32 * 8.0 / dt / 1000.0) as u32,
                    frames_per_sec: frames as f32 / dt,
                    dropped_percent,
                    quality: NetQuality::rate(rtt, jitter, dropped_percent),
                    host: host_stats.clone(),
                    latency: LatencyView {
                        input_rtt_ms: tx.counters.input_rtt_us.load(Ordering::Relaxed) as f32 / 1000.0,
                        input_queue_ms: tx.counters.input_queue_us.load(Ordering::Relaxed) as f32 / 1000.0,
                        host_queue_ms: host_stats.input_queue_us as f32 / 1000.0,
                        host_apply_ms: host_stats.input_apply_us as f32 / 1000.0,
                        motion_to_frame_ms: tx.counters.motion_to_frame_us.load(Ordering::Relaxed) as f32 / 1000.0,
                        stale_moves_dropped: tx.counters.moves_coalesced.load(Ordering::Relaxed) + host_stats.stale_moves_dropped,
                        stale_video_dropped: 0,
                        file_rate_kbps: (tx.counters.bulk_rate_bps.load(Ordering::Relaxed) / 125) as u32,
                    },
                };
                let _ = events.send(Event::ViewerStats { session: session_no, stats });
                (frames, delivered_win, last_bytes, last_tick) = (0, 0, bytes, Instant::now());
            }
            _ = s.closed.changed() => break EndReason::ConnectionLost,
        }
    };
    reason
}
