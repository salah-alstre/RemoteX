//! Windows implementations of the platform traits: DXGI capture, Media Foundation encoding,
//! SendInput injection and the system clipboard.

use crate::{
    engine::is_lan_candidate,
    env::{
        BoxedClipboard, EncodedUnit, EncoderFactory, EncoderRequest, Frame, InputSink, PlatformEnv, Pointer,
        ScreenSource, SourceError, SourceOutput, VideoEncoder,
    },
};
use remotex_capture::{list_monitors, CaptureError, Capturer, CursorTracker, Monitor};
use remotex_codec::{available_encoders, Encoder, EncoderConfig};
use remotex_common::peer::{Codec, CursorShape, DisplayInfo, InputEvent};
use remotex_input::{injector::Injector, system_clipboard::SystemClipboard};
use std::{net::IpAddr, sync::Arc};

pub fn env() -> PlatformEnv {
    PlatformEnv {
        screen: Arc::new(|| WinScreen::new().map(|s| Box::new(s) as Box<dyn ScreenSource>)),
        encoders: Arc::new(MfEncoders),
        input: Arc::new(|| Box::new(WinInput { inj: Injector::new() })),
        clipboard: Arc::new(|images| {
            SystemClipboard::new(images)
                .ok()
                .map(|c| Box::new(c) as BoxedClipboard)
        }),
        local_addresses: Arc::new(local_addresses),
        elevation: Arc::new(crate::elevation::ServiceElevation::new()),
    }
}

/// Like [`env`], but talks to a service on a different pipe (development builds and tests only).
pub fn env_with_service_pipe(pipe: &str) -> PlatformEnv {
    PlatformEnv {
        elevation: Arc::new(crate::elevation::ServiceElevation::with_pipe(pipe)),
        ..env()
    }
}

fn to_display(m: &Monitor) -> DisplayInfo {
    DisplayInfo {
        index: m.index,
        name: format!("Display {} ({})", m.index + 1, m.name),
        width: m.width,
        height: m.height,
        x: m.x,
        y: m.y,
        primary: m.primary,
    }
}

struct WinScreen {
    monitors: Vec<Monitor>,
    capturer: Option<Capturer>,
    cursor: CursorTracker,
}

impl WinScreen {
    fn new() -> Result<Self, String> {
        remotex_capture::enable_dpi_awareness();
        let monitors = list_monitors().map_err(|e| e.to_string())?;
        if monitors.is_empty() {
            return Err("no displays found".into());
        }
        Ok(Self {
            monitors,
            capturer: None,
            cursor: CursorTracker::default(),
        })
    }
}

impl ScreenSource for WinScreen {
    fn displays(&mut self) -> Vec<DisplayInfo> {
        if let Ok(m) = list_monitors() {
            if !m.is_empty() {
                self.monitors = m;
            }
        }
        self.monitors.iter().map(to_display).collect()
    }

    fn select(&mut self, index: u32) -> Result<DisplayInfo, String> {
        let m = self
            .monitors
            .get(index as usize)
            .ok_or("no such display")?
            .clone();
        self.capturer = Some(Capturer::new(&m).map_err(|e| e.to_string())?);
        self.cursor.reset();
        Ok(to_display(&m))
    }

    fn next(&mut self, out_w: u32, out_h: u32, timeout_ms: u32) -> Result<SourceOutput, SourceError> {
        let cap = self
            .capturer
            .as_mut()
            .ok_or_else(|| SourceError::Fatal("no display selected".into()))?;
        let got = cap.next(out_w, out_h, timeout_ms).map_err(|e| match e {
            CaptureError::AccessLost => SourceError::Lost,
            other => SourceError::Fatal(other.to_string()),
        })?;
        let shape = self.cursor.poll().map(|c| CursorShape {
            id: c.id,
            width: c.width,
            height: c.height,
            hot_x: c.hot_x,
            hot_y: c.hot_y,
            rgba: c.rgba,
        });
        Ok(SourceOutput {
            frame: got.frame.map(|f| Frame {
                width: f.width,
                height: f.height,
                data: f.data,
            }),
            pointer: Some(Pointer {
                x: got.pointer.x,
                y: got.pointer.y,
                visible: got.pointer.visible,
            }),
            cursor_shape: shape,
            convert_us: got.convert_us,
        })
    }

    fn request_frame(&mut self) {
        if let Some(c) = self.capturer.as_mut() {
            c.request_frame();
        }
    }
}

struct MfEncoders;

struct MfEncoder {
    inner: Encoder,
    codec: Codec,
}

impl EncoderFactory for MfEncoders {
    fn create(&self, req: &EncoderRequest) -> Result<Box<dyn VideoEncoder>, String> {
        let inner = Encoder::new(EncoderConfig {
            codec: req.codec,
            width: req.width,
            height: req.height,
            fps: req.fps,
            bitrate_kbps: req.bitrate_kbps,
            prefer_hardware: req.hardware,
        })
        .map_err(|e| e.to_string())?;
        Ok(Box::new(MfEncoder {
            inner,
            codec: req.codec,
        }))
    }

    fn available(&self) -> Vec<(Codec, String, bool)> {
        available_encoders()
            .into_iter()
            .map(|e| (e.codec, e.name, e.hardware))
            .collect()
    }
}

impl VideoEncoder for MfEncoder {
    fn encode(&mut self, frame: &Frame, force_key: bool) -> Result<Vec<EncodedUnit>, String> {
        if force_key {
            self.inner.force_keyframe();
        }
        let out = self.inner.encode(&frame.data).map_err(|e| e.to_string())?;
        Ok(out
            .into_iter()
            .map(|f| EncodedUnit {
                data: f.data,
                keyframe: f.keyframe,
            })
            .collect())
    }

    fn set_bitrate(&mut self, kbps: u32) {
        self.inner.set_bitrate(kbps);
    }

    fn name(&self) -> String {
        self.inner.name().to_string()
    }

    fn codec(&self) -> Codec {
        self.codec
    }
}

struct WinInput {
    inj: Injector,
}

impl InputSink for WinInput {
    fn apply(&mut self, event: &InputEvent, display: &DisplayInfo) {
        self.inj.apply(event, display);
    }

    fn release_all(&mut self) {
        self.inj.release_all();
    }
}

/// Primary LAN address, found by asking the OS which interface it would route to the internet
/// through (a UDP "connect" sends no packets).
fn local_addresses() -> Vec<IpAddr> {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("8.8.8.8:80").map(|_| s))
        .and_then(|s| s.local_addr())
        .map(|a| a.ip())
        .into_iter()
        .filter(is_lan_candidate)
        .collect()
}
