//! Host video pipeline on a dedicated thread: capture -> encode -> prioritised send queue.
//! Capture and encoder objects have thread affinity, so they are created and used only here.

use crate::{
    adapt::{output_size, Adapter, Target},
    env::{
        ElevatedVideo, EncoderRequest, PlatformEnv, ScreenSource, SourceError, SourceOutput, VideoEncoder,
    },
    secure::SecureTx,
};
use remotex_common::peer::{
    Codec, CursorShape, CursorUpdate, DisplayInfo, HostStats, Msg, QualitySettings, VideoFrame,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc as std_mpsc, Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

pub enum VideoCtl {
    Select(u32),
    Quality(QualitySettings),
    Keyframe,
    Target(Target),
    Pause(bool),
    /// Elevated control was granted (or ended): pictures of the secure desktop come from the service.
    Elevated(Option<Box<dyn ElevatedVideo>>),
}

pub enum VideoEvent {
    Stats(HostStats),
    Displays(Vec<DisplayInfo>, u32),
    Failed(String),
}

pub struct VideoThread {
    ctl: std_mpsc::Sender<VideoCtl>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl VideoThread {
    pub fn control(&self, c: VideoCtl) {
        let _ = self.ctl.send(c);
    }
}

impl Drop for VideoThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

pub fn spawn(
    env: PlatformEnv,
    tx: SecureTx,
    quality: QualitySettings,
    display: u32,
    start_paused: bool,
    events: mpsc::UnboundedSender<VideoEvent>,
) -> VideoThread {
    let (ctl_tx, ctl_rx) = std_mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let join = std::thread::Builder::new()
        .name("video-pipeline".into())
        .spawn(move || {
            if let Err(e) = run(env, tx, quality, display, start_paused, ctl_rx, &stop2, &events) {
                let _ = events.send(VideoEvent::Failed(e));
            }
        })
        .expect("spawn video thread");
    VideoThread {
        ctl: ctl_tx,
        stop,
        join: Some(join),
    }
}

struct EncoderSlot {
    enc: Box<dyn VideoEncoder>,
    width: u32,
    height: u32,
    fps: u32,
    codec: Codec,
}

fn make_encoder(
    env: &PlatformEnv,
    quality: &QualitySettings,
    w: u32,
    h: u32,
    t: Target,
) -> Result<EncoderSlot, String> {
    let wanted = quality.codec.unwrap_or(Codec::H264);
    let mut last = String::new();
    for codec in [wanted, Codec::H264] {
        let req = EncoderRequest {
            codec,
            width: w,
            height: h,
            fps: t.fps,
            bitrate_kbps: t.bitrate_kbps,
            hardware: quality.hardware,
        };
        match env.encoders.create(&req) {
            Ok(enc) => {
                return Ok(EncoderSlot {
                    enc,
                    width: w,
                    height: h,
                    fps: t.fps,
                    codec,
                })
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

#[allow(clippy::too_many_arguments)]
fn run(
    env: PlatformEnv,
    tx: SecureTx,
    mut quality: QualitySettings,
    display: u32,
    start_paused: bool,
    ctl: std_mpsc::Receiver<VideoCtl>,
    stop: &AtomicBool,
    events: &mpsc::UnboundedSender<VideoEvent>,
) -> Result<(), String> {
    let mut source: Box<dyn ScreenSource> = (env.screen)()?;
    let mut displays = source.displays();
    let mut active = source.select(display.min(displays.len().saturating_sub(1) as u32))?;
    let mut adapter = Adapter::new(quality, active.width, active.height, Instant::now());
    let mut target = adapter.target();
    let mut slot: Option<EncoderSlot> = None;
    let mut force_key = true;
    let mut paused = start_paused;

    let start = Instant::now();
    let mut seq = 0u64;
    let mut last_sent = Instant::now() - Duration::from_secs(1);
    let mut last_displays_check = Instant::now();
    let mut last_pointer = None;
    let mut last_pointer_sent = Instant::now();
    // While the person is actively controlling the desktop, capture at the highest rate so the effect of
    // an input event is on its way as soon as possible; idle desktops follow the (adaptive) target rate.
    let mut elevated: Option<Box<dyn ElevatedVideo>> = None;
    let mut use_service_picture = false;
    let mut last_probe = Instant::now();
    let mut last_input_seq = tx.counters.applied_input_seq.load(Ordering::Relaxed);
    let mut last_input_activity = Instant::now() - Duration::from_secs(10);

    // Rolling stats window.
    let mut win_start = Instant::now();
    let (mut win_frames, mut win_bytes, mut win_capture_us, mut win_encode_us, mut win_samples) =
        (0u32, 0u64, 0u64, 0u64, 0u32);

    while !stop.load(Ordering::Relaxed) {
        while let Ok(c) = ctl.try_recv() {
            match c {
                VideoCtl::Select(i) => match source.select(i) {
                    Ok(d) => {
                        active = d;
                        adapter = Adapter::new(quality, active.width, active.height, Instant::now());
                        target = adapter.target();
                        slot = None;
                        force_key = true;
                        let _ = events.send(VideoEvent::Displays(displays.clone(), active.index));
                    }
                    Err(e) => tracing::warn!(target: "capture", error = %e, "display switch failed"),
                },
                VideoCtl::Quality(q) => {
                    quality = q;
                    adapter = Adapter::new(quality, active.width, active.height, Instant::now());
                    target = adapter.target();
                    slot = None;
                    force_key = true;
                }
                VideoCtl::Keyframe => {
                    force_key = true;
                    source.request_frame();
                }
                VideoCtl::Target(t) => {
                    if t.fps != target.fps || t.scale_percent != target.scale_percent {
                        slot = None;
                        force_key = true;
                    } else if let Some(s) = slot.as_mut() {
                        s.enc.set_bitrate(t.bitrate_kbps);
                    }
                    target = t;
                }
                VideoCtl::Elevated(e) => {
                    elevated = e;
                    use_service_picture = false;
                    force_key = true;
                    source.request_frame();
                }
                VideoCtl::Pause(p) => {
                    paused = p;
                    if !p {
                        force_key = true;
                        source.request_frame();
                    }
                }
            }
        }
        if paused {
            std::thread::sleep(Duration::from_millis(30));
            continue;
        }

        if last_displays_check.elapsed() > Duration::from_secs(3) {
            last_displays_check = Instant::now();
            let now = source.displays();
            if now != displays {
                displays = now;
                let _ = events.send(VideoEvent::Displays(displays.clone(), active.index));
            }
        }

        let (w, h) = output_size(
            active.width,
            active.height,
            target.scale_percent,
            quality.max_height,
        );
        if slot
            .as_ref()
            .is_none_or(|s| (s.width, s.height, s.fps) != (w, h, target.fps))
        {
            slot = Some(make_encoder(&env, &quality, w, h, target)?);
            force_key = true;
            source.request_frame();
        }
        let s = slot.as_mut().expect("encoder present");

        let seq_now = tx.counters.applied_input_seq.load(Ordering::Relaxed);
        if seq_now != last_input_seq {
            last_input_seq = seq_now;
            last_input_activity = Instant::now();
        }
        let base_interval = Duration::from_micros(1_000_000 / target.fps.max(1) as u64);
        let interval = if last_input_activity.elapsed() < Duration::from_millis(600) {
            base_interval.min(Duration::from_micros(16_667))
        } else {
            base_interval
        };
        let since = last_sent.elapsed();
        if since < interval {
            std::thread::sleep(interval - since);
        }

        let t0 = Instant::now();
        // Input applied before this instant is (at least partly) visible in the frame captured next.
        let captured_input_seq = tx.counters.applied_input_seq.load(Ordering::Relaxed);

        // Elevated session: ask whether a UAC prompt (secure desktop) is in front. Ordinary capture cannot
        // see it, so its pictures come from the service instead, until the prompt is gone.
        if let Some(e) = elevated.as_mut() {
            if last_probe.elapsed() > Duration::from_millis(250) {
                last_probe = Instant::now();
                let secure = e.secure_desktop();
                if secure != use_service_picture {
                    use_service_picture = secure;
                    force_key = true;
                    source.request_frame();
                    tracing::info!(target: "capture", secure, "switched picture source for the secure desktop");
                }
            }
        }
        let service_picture = if use_service_picture {
            elevated.as_mut().and_then(|e| e.capture(&active, w, h))
        } else {
            None
        };
        let next = if use_service_picture {
            match service_picture {
                Some(p) => Ok(SourceOutput {
                    frame: Some(p.frame),
                    pointer: p.pointer,
                    cursor_shape: None,
                    convert_us: 0,
                }),
                None => {
                    std::thread::sleep(Duration::from_millis(30));
                    continue;
                }
            }
        } else {
            source.next(w, h, interval.as_millis().max(1) as u32)
        };
        let out = match next {
            Ok(o) => o,
            Err(SourceError::Lost) => {
                // Ordinary capture loses the desktop exactly when a UAC prompt appears; with an elevated
                // session the service can still see it.
                if let Some(e) = elevated.as_mut() {
                    if e.secure_desktop() {
                        use_service_picture = true;
                        force_key = true;
                        continue;
                    }
                }
                std::thread::sleep(Duration::from_millis(250));
                source = (env.screen)()?;
                displays = source.displays();
                active = source.select(active.index.min(displays.len().saturating_sub(1) as u32))?;
                slot = None;
                continue;
            }
            Err(SourceError::Fatal(e)) => return Err(e),
        };

        if let Some(p) = out.pointer {
            if last_pointer != Some(p)
                && (last_pointer_sent.elapsed() > Duration::from_millis(8) || out.cursor_shape.is_some())
            {
                last_pointer = Some(p);
                last_pointer_sent = Instant::now();
                let shape: Option<CursorShape> = out.cursor_shape;
                tx.send(Msg::Cursor(CursorUpdate {
                    x: p.x,
                    y: p.y,
                    visible: p.visible,
                    shape,
                }));
            }
        } else if let Some(shape) = out.cursor_shape {
            let p = last_pointer.unwrap_or_default();
            tx.send(Msg::Cursor(CursorUpdate {
                x: p.x,
                y: p.y,
                visible: p.visible,
                shape: Some(shape),
            }));
        }

        let Some(frame) = out.frame else { continue };
        if tx.video_congested() {
            // The network is behind: skip this frame but make sure the final state is still delivered.
            tx.counters.video_dropped.fetch_add(1, Ordering::Relaxed);
            source.request_frame();
            std::thread::sleep(Duration::from_millis(4));
            continue;
        }
        win_capture_us += t0.elapsed().as_micros() as u64;
        let t1 = Instant::now();
        let units = match s.enc.encode(&frame, std::mem::take(&mut force_key)) {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!(target: "codec", error = %e, "encode failed, restarting encoder");
                slot = None;
                continue;
            }
        };
        win_encode_us += t1.elapsed().as_micros() as u64;
        win_samples += 1;
        last_sent = Instant::now();
        for u in units {
            seq += 1;
            win_bytes += u.data.len() as u64;
            win_frames += 1;
            let msg = Msg::Video(VideoFrame {
                seq,
                display: active.index,
                codec: s.codec,
                keyframe: u.keyframe,
                width: w,
                height: h,
                captured_ms: start.elapsed().as_millis() as u64,
                input_seq: captured_input_seq,
                data: u.data,
            });
            if !tx.try_send_video(msg) {
                force_key = true;
            }
        }
        if tx.is_closed() {
            break;
        }

        if win_start.elapsed() >= Duration::from_secs(1) {
            let dt = win_start.elapsed().as_secs_f32();
            let n = win_samples.max(1) as f32;
            let _ = events.send(VideoEvent::Stats(HostStats {
                fps: win_frames as f32 / dt,
                bitrate_kbps: (win_bytes as f32 * 8.0 / dt / 1000.0) as u32,
                capture_ms: win_capture_us as f32 / n / 1000.0,
                encode_ms: win_encode_us as f32 / n / 1000.0,
                dropped: tx.counters.video_dropped.load(Ordering::Relaxed),
                encoder: s.enc.name(),
                codec: Some(s.codec),
                width: s.width,
                height: s.height,
                input_queue_us: 0,
                input_apply_us: 0,
                stale_moves_dropped: 0,
            }));
            win_start = Instant::now();
            (win_frames, win_bytes, win_capture_us, win_encode_us, win_samples) = (0, 0, 0, 0, 0);
        }
    }
    Ok(())
}
