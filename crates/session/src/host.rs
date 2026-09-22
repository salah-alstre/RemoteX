//! Host side of an authenticated session: streams the desktop, applies permitted input,
//! synchronises the clipboard, serves files, and enforces the permissions the owner granted.

use crate::{
    adapt::{Adapter, Sample},
    env::PlatformEnv,
    handshake::Authenticated,
    input_thread::{self, InputShared},
    link::Link,
    secure::{self, InboundInput, SecureTx},
    transfer::{FileManager, FilePolicy, Side},
    types::{EndReason, EngineConfig, Event, EventTx, LinkInfo},
    video::{self, VideoCtl, VideoEvent},
};
use remotex_common::peer::{
    ClipboardPayload, DisplayInfo, InputEvent, Msg, Permissions, QualitySettings, RejectReason,
};
use remotex_input::ClipboardSync;
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

pub enum HostCmd {
    SetPermissions(Permissions),
    Chat(String),
    Kick,
    ApproveFile { id: u64, accept: bool },
}

#[derive(Clone)]
pub struct HostHandle {
    pub cmd: mpsc::UnboundedSender<HostCmd>,
}

pub struct HostSession {
    pub session_no: u64,
    pub env: PlatformEnv,
    pub events: EventTx,
    pub cfg: Arc<EngineConfig>,
    pub host_id: String,
    pub host_name: String,
    pub auth: Authenticated,
    pub link: Link,
    pub granted: Permissions,
    pub cmd_rx: mpsc::UnboundedReceiver<HostCmd>,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Runs until the session ends and returns why. Always releases held input on exit.
pub async fn run(s: HostSession) -> EndReason {
    let HostSession {
        session_no,
        env,
        events,
        cfg,
        host_name,
        auth,
        link,
        granted,
        mut cmd_rx,
        ..
    } = s;
    let link_info = LinkInfo {
        kind: link.kind,
        peer: link.peer.clone(),
        protocol: match link.kind {
            crate::link::LinkKind::Direct => "tcp",
            crate::link::LinkKind::Relay => "tls-websocket",
        },
    };
    let (auth_viewer, kind) = (auth.viewer_id.clone(), link.kind);
    let (route_tx, route_rx) = std::sync::mpsc::channel::<InboundInput>();
    let (tx, mut rx, mut closed) = secure::spawn(link, auth.channel, Some(route_tx.clone()));

    let mut perms = granted;
    let mut max_perms = granted;
    // Elevated control needs the RemoteX service. If it is not there, the session simply runs without it
    // (and both sides are told): a permission that cannot be honoured is never reported as granted.
    let mut elevated_session = None;
    if perms.elevated {
        match env.elevation.begin(&auth.viewer_name) {
            Ok(s) => elevated_session = Some(s),
            Err(e) => {
                tracing::warn!(target: "security", error = %e, "elevated control unavailable, continuing without it");
                perms.elevated = false;
                max_perms.elevated = false;
                let _ = events.send(Event::HostNotice {
                    notice: crate::types::HostNotice::ElevationUnavailable,
                });
            }
        }
    }

    // Screen source is probed here only to report displays; the video thread owns its own instance.
    let displays: Vec<DisplayInfo> = match (env.screen)() {
        Ok(mut src) => src.displays(),
        Err(e) => {
            tracing::error!(target: "capture", error = %e, "cannot capture the screen");
            tx.send(Msg::Rejected(RejectReason::Busy));
            return EndReason::ConnectionLost;
        }
    };
    let mut displays = displays;
    let mut active = displays.iter().position(|d| d.primary).unwrap_or(0) as u32;
    // Input is applied on its own thread, fed directly by the network reader.
    let shared = InputShared::new(perms, displays.clone(), active);
    let input = input_thread::spawn((env.input)(), shared.clone(), route_rx, tx.clone());
    let mut elevated_guard: Option<Box<dyn Send>> = None;
    let mut pending_video = None;
    if let Some(s) = elevated_session.take() {
        shared.set_elevated(Some(s.input));
        pending_video = Some(s.video);
        elevated_guard = Some(s.guard);
    }
    let release_all = |route: &std::sync::mpsc::Sender<InboundInput>| {
        let _ = route.send(InboundInput {
            seq: 0,
            ev: InputEvent::ReleaseAll,
            arrived: Instant::now(),
        });
    };

    tx.send(Msg::Accepted {
        permissions: perms,
        displays: displays.clone(),
        active_display: active,
        host_name,
    });
    let _ = events.send(Event::HostStarted {
        viewer_id: auth.viewer_id.clone(),
        viewer_name: auth.viewer_name.clone(),
        permissions: perms,
        link: link_info,
    });

    let (vid_tx, mut vid_rx) = mpsc::unbounded_channel();
    let video = video::spawn(
        env.clone(),
        tx.clone(),
        QualitySettings::default(),
        active,
        !perms.view,
        vid_tx,
    );
    if let Some(v) = pending_video.take() {
        video.control(VideoCtl::Elevated(Some(v)));
    }
    let mut quality = QualitySettings::default();
    let mut adapter = displays
        .get(active as usize)
        .map(|d| Adapter::new(quality, d.width, d.height, Instant::now()));

    let files = FileManager::new(
        FilePolicy {
            side: Side::Host,
            download_dir: cfg.download_dir.clone(),
            ask_before_receiving: cfg.ask_before_receiving,
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

    let mut ping = tokio::time::interval(Duration::from_secs(2));
    // While a file moves, tiny extra pings (10 per second) measure queueing delay so its rate can follow it.
    let mut probe = tokio::time::interval(Duration::from_millis(100));
    let mut last_bulk = 0u64;
    let mut governor = crate::bulk::BulkGovernor::default();
    let mut clip_tick = tokio::time::interval(Duration::from_millis(300));
    let mut adapt_tick = tokio::time::interval(Duration::from_secs(1));
    let mut last_rx = Instant::now();
    let mut rtt_ms = 0f32;
    let mut last_dropped = 0u64;

    tracing::info!(target: "session", role = "host", viewer = %auth_viewer, link = ?kind, "remote session started");
    let reason = loop {
        tokio::select! {
            msg = rx.recv() => {
                let Some(msg) = msg else { break EndReason::ConnectionLost };
                last_rx = Instant::now();
                match msg {
                    Msg::SelectDisplay(i) if perms.view => {
                        if (i as usize) < displays.len() {
                            active = i;
                            shared.set_active(i);
                            video.control(VideoCtl::Select(i));
                            adapter = displays.get(i as usize).map(|d| Adapter::new(quality, d.width, d.height, Instant::now()));
                        }
                    }
                    Msg::SetQuality(q) if perms.view => {
                        quality = q;
                        video.control(VideoCtl::Quality(q));
                        adapter = displays.get(active as usize).map(|d| Adapter::new(q, d.width, d.height, Instant::now()));
                    }
                    Msg::RequestKeyframe if perms.view => video.control(VideoCtl::Keyframe),
                    Msg::Clipboard(p) if perms.clipboard => {
                        if let Some(c) = clipboard.as_mut() {
                            if let Err(e) = c.apply_remote(&p) {
                                tracing::debug!(error = %e, "remote clipboard update rejected");
                            }
                        }
                    }
                    Msg::File(f) => files.on_msg(f).await,
                    Msg::Chat { id, text, ts_ms } => {
                        let _ = events.send(Event::Chat { session: session_no, id, text, ts_ms, mine: false });
                    }
                    Msg::Ping { t } => { tx.send(Msg::Pong { t }); }
                    Msg::Pong { t } => {
                        rtt_ms = now_ms().saturating_sub(t) as f32;
                        tx.counters.bulk_rate_bps.store(governor.update(rtt_ms), Ordering::Relaxed);
                    }
                    Msg::Bye => break EndReason::PeerClosed,
                    // Host-to-viewer message types or permission-denied requests are ignored.
                    _ => {}
                }
            }
            Some(cmd) = cmd_rx.recv() => match cmd {
                HostCmd::SetPermissions(p) => {
                    let mut new = p.intersect(max_perms);
                    // Elevated control can be switched off and on again during a session (the owner is
                    // in control), but never on unless it was granted at the start.
                    if new.elevated && !perms.elevated {
                        match env.elevation.begin(&auth_viewer) {
                            Ok(s) => {
                                shared.set_elevated(Some(s.input));
                                video.control(VideoCtl::Elevated(Some(s.video)));
                                elevated_guard = Some(s.guard);
                            }
                            Err(e) => {
                                tracing::warn!(target: "security", error = %e, "elevated control unavailable");
                                new.elevated = false;
                            }
                        }
                    } else if !new.elevated && perms.elevated {
                        shared.set_elevated(None);
                        video.control(VideoCtl::Elevated(None));
                        elevated_guard = None; // revokes at the service at once
                    }
                    if new != perms {
                        let lost_input = (perms.mouse && !new.mouse) || (perms.keyboard && !new.keyboard);
                        shared.set_permissions(new);
                        if lost_input { release_all(&route_tx); }
                        if new.view != perms.view { video.control(VideoCtl::Pause(!new.view)); }
                        files.set_allowed(new.files);
                        perms = new;
                        tx.send(Msg::PermissionsChanged(perms));
                        let _ = events.send(Event::HostPermissions(perms));
                    }
                }
                HostCmd::Chat(text) => {
                    let (id, ts_ms) = (now_ms(), now_ms());
                    tx.send(Msg::Chat { id, text: text.clone(), ts_ms });
                    let _ = events.send(Event::Chat { session: session_no, id, text, ts_ms, mine: true });
                }
                HostCmd::ApproveFile { id, accept } => files.respond(id, accept),
                HostCmd::Kick => {
                    tx.send(Msg::Bye);
                    // Let the writer flush the goodbye before tearing down.
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    break EndReason::Kicked;
                }
            },
            Some(ev) = vid_rx.recv() => match ev {
                VideoEvent::Stats(mut st) => {
                    st.input_queue_us = shared.queue_us.load(Ordering::Relaxed);
                    st.input_apply_us = shared.apply_us.load(Ordering::Relaxed);
                    st.stale_moves_dropped = shared.stale_moves.load(Ordering::Relaxed);
                    tx.send(Msg::Stats(st));
                }
                VideoEvent::Displays(list, act) => {
                    displays = list.clone();
                    active = act;
                    shared.set_displays(list.clone(), act);
                    tx.send(Msg::DisplaysChanged { displays: list, active: act });
                }
                VideoEvent::Failed(e) => {
                    tracing::error!(target: "capture", error = %e, "video pipeline stopped");
                    break EndReason::ConnectionLost;
                }
            },
            _ = clip_tick.tick(), if perms.clipboard => {
                if let Some(c) = clipboard.as_mut() {
                    if let Some(p) = c.poll_local() {
                        match p {
                            ClipboardPayload::Image(_) => { tx.send_bulk(Msg::Clipboard(p)).await; }
                            text => { tx.send(Msg::Clipboard(text)); }
                        }
                    }
                }
            }
            _ = adapt_tick.tick() => {
                // The service can end a grant on its own (it lapsed, was stopped, or the helper died).
                if perms.elevated && shared.take_elevated_lost() {
                    tracing::warn!(target: "security", "elevated control ended by the service");
                    video.control(VideoCtl::Elevated(None));
                    elevated_guard = None;
                    perms.elevated = false;
                    max_perms.elevated = false;
                    tx.send(Msg::PermissionsChanged(perms));
                    let _ = events.send(Event::HostPermissions(perms));
                }
                let dropped = tx.counters.video_dropped.load(Ordering::Relaxed);
                let sample = Sample {
                    rtt_ms,
                    send_delay_ms: tx.counters.video_send_us.load(Ordering::Relaxed) as f32 / 1000.0,
                    dropped: dropped.saturating_sub(last_dropped) as u32,
                };
                last_dropped = dropped;
                if let Some(t) = adapter.as_mut().and_then(|a| a.update(sample, Instant::now())) {
                    video.control(VideoCtl::Target(t));
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
            }
            _ = closed.changed() => break EndReason::ConnectionLost,
        }
    };

    tracing::info!(target: "session", role = "host", ?reason, "remote session ended");
    // Releases every held key and button before the session is reported as over.
    input.finish();
    // The session is over: elevated authorization goes with it, immediately.
    video.control(VideoCtl::Elevated(None));
    drop(elevated_guard.take());
    if reason != EndReason::PeerClosed && reason != EndReason::Kicked {
        tx.send(Msg::Bye);
    }
    drop(video);
    reason
}

/// Sends a Rejected message on a freshly authenticated channel and closes it.
pub async fn reject(auth: Authenticated, link: Link, reason: RejectReason) {
    let (tx, _rx, _closed): (SecureTx, _, _) = secure::spawn(link, auth.channel, None);
    tx.send(Msg::Rejected(reason));
    tokio::time::sleep(Duration::from_millis(200)).await;
}
