//! In-memory platform implementation so the full protocol (server, handshake, streaming, input,
//! clipboard, files, reconnect) runs in tests without a desktop, GPU or real clipboard.

use crate::{
    env::{
        BoxedClipboard, ElevatedInput, ElevatedPicture, ElevatedSession, ElevatedVideo, ElevationProvider,
        EncodedUnit, EncoderFactory, EncoderRequest, Frame, InputSink, PlatformEnv, Pointer, ScreenSource,
        SourceError, SourceOutput, VideoEncoder,
    },
    signaling::{CredentialStore, Credentials},
};
use remotex_common::peer::{ClipboardPayload, Codec, DisplayInfo, InputEvent};
use remotex_input::ClipboardBackend;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

/// Observable state of the mock elevated-control service.
pub struct MockElevationState {
    pub available: AtomicBool,
    pub begun: AtomicUsize,
    pub revoked: AtomicUsize,
    /// Simulates a UAC prompt being in front.
    pub secure: AtomicBool,
    pub inputs: Mutex<Vec<InputEvent>>,
}

impl Default for MockElevationState {
    fn default() -> Self {
        Self {
            available: AtomicBool::new(true),
            begun: AtomicUsize::new(0),
            revoked: AtomicUsize::new(0),
            secure: AtomicBool::new(false),
            inputs: Mutex::new(Vec::new()),
        }
    }
}

struct MockElevation(Arc<MockElevationState>);
struct MockElevatedInput(Arc<MockElevationState>, Arc<AtomicBool>);
struct MockElevatedVideo(Arc<MockElevationState>);
struct MockGuard(Arc<MockElevationState>, Arc<AtomicBool>);

impl ElevationProvider for MockElevation {
    fn available(&self) -> bool {
        self.0.available.load(Ordering::SeqCst)
    }
    fn begin(&self, _label: &str) -> Result<ElevatedSession, String> {
        if !self.available() {
            return Err("service not installed".into());
        }
        self.0.begun.fetch_add(1, Ordering::SeqCst);
        let alive = Arc::new(AtomicBool::new(true));
        Ok(ElevatedSession {
            input: Box::new(MockElevatedInput(self.0.clone(), alive.clone())),
            video: Box::new(MockElevatedVideo(self.0.clone())),
            guard: Box::new(MockGuard(self.0.clone(), alive)),
        })
    }
}

impl ElevatedInput for MockElevatedInput {
    fn apply(&mut self, event: &InputEvent, _display: &DisplayInfo) {
        self.0.inputs.lock().unwrap().push(event.clone());
    }
    fn alive(&self) -> bool {
        self.1.load(Ordering::SeqCst)
    }
}

impl ElevatedVideo for MockElevatedVideo {
    fn secure_desktop(&mut self) -> bool {
        self.0.secure.load(Ordering::SeqCst)
    }
    fn capture(&mut self, _d: &DisplayInfo, w: u32, h: u32) -> Option<ElevatedPicture> {
        Some(ElevatedPicture {
            // A recognisable picture so tests can tell secure-desktop frames from ordinary ones.
            frame: Frame {
                width: w,
                height: h,
                data: vec![0xEE; (w * h * 3 / 2) as usize],
            },
            pointer: None,
        })
    }
}

impl Drop for MockGuard {
    fn drop(&mut self) {
        self.1.store(false, Ordering::SeqCst);
        self.0.revoked.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
pub struct MemoryStore(Mutex<Option<Credentials>>);

impl CredentialStore for MemoryStore {
    fn load(&self) -> Option<Credentials> {
        self.0.lock().unwrap().clone()
    }
    fn save(&self, creds: &Credentials) {
        *self.0.lock().unwrap() = Some(creds.clone());
    }
    fn clear(&self) {
        *self.0.lock().unwrap() = None;
    }
}

#[derive(Default)]
pub struct SharedClipboard {
    pub seq: u32,
    pub content: Option<ClipboardPayload>,
}

impl SharedClipboard {
    /// Simulates the user copying something locally.
    pub fn user_copy(this: &Arc<Mutex<Self>>, text: &str) {
        let mut g = this.lock().unwrap();
        g.seq += 1;
        g.content = Some(ClipboardPayload::Text(text.into()));
    }

    pub fn text(this: &Arc<Mutex<Self>>) -> Option<String> {
        match &this.lock().unwrap().content {
            Some(ClipboardPayload::Text(t)) => Some(t.clone()),
            _ => None,
        }
    }
}

struct MockClipboard(Arc<Mutex<SharedClipboard>>);

impl ClipboardBackend for MockClipboard {
    fn sequence(&self) -> u32 {
        self.0.lock().unwrap().seq
    }
    fn read(&mut self) -> Option<ClipboardPayload> {
        self.0.lock().unwrap().content.clone()
    }
    fn write(&mut self, p: &ClipboardPayload) -> Result<(), String> {
        let mut g = self.0.lock().unwrap();
        g.seq += 1;
        g.content = Some(p.clone());
        Ok(())
    }
}

pub type InputLog = Arc<Mutex<Vec<(InputEvent, u32)>>>;

struct MockInput(InputLog);

impl InputSink for MockInput {
    fn apply(&mut self, event: &InputEvent, display: &DisplayInfo) {
        self.0.lock().unwrap().push((event.clone(), display.index));
    }
    fn release_all(&mut self) {
        self.0.lock().unwrap().push((InputEvent::ReleaseAll, u32::MAX));
    }
}

struct MockScreen {
    displays: Vec<DisplayInfo>,
    active: usize,
    counter: u8,
    forced: bool,
}

impl ScreenSource for MockScreen {
    fn displays(&mut self) -> Vec<DisplayInfo> {
        self.displays.clone()
    }
    fn select(&mut self, index: u32) -> Result<DisplayInfo, String> {
        self.active = index as usize;
        self.displays
            .get(self.active)
            .cloned()
            .ok_or_else(|| "no such display".into())
    }
    fn next(&mut self, out_w: u32, out_h: u32, timeout_ms: u32) -> Result<SourceOutput, SourceError> {
        std::thread::sleep(Duration::from_millis(timeout_ms.min(8) as u64));
        self.counter = self.counter.wrapping_add(1);
        self.forced = false;
        Ok(SourceOutput {
            frame: Some(Frame {
                width: out_w,
                height: out_h,
                data: vec![self.counter; (out_w * out_h * 3 / 2) as usize],
            }),
            pointer: Some(Pointer {
                x: 10,
                y: 20,
                visible: true,
            }),
            cursor_shape: None,
            convert_us: 100,
        })
    }
    fn request_frame(&mut self) {
        self.forced = true;
    }
}

struct MockEncoder {
    codec: Codec,
    frames: u32,
}

impl VideoEncoder for MockEncoder {
    fn encode(&mut self, frame: &Frame, force_key: bool) -> Result<Vec<EncodedUnit>, String> {
        let key = force_key || self.frames == 0;
        self.frames += 1;
        // Recognisable Annex-B-like payload: SPS+PPS+IDR for keyframes, a slice otherwise.
        let mut data = if key {
            vec![0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65]
        } else {
            vec![0, 0, 0, 1, 0x41]
        };
        data.extend_from_slice(&frame.data[..frame.data.len().min(512)]);
        Ok(vec![EncodedUnit { data, keyframe: key }])
    }
    fn set_bitrate(&mut self, _kbps: u32) {}
    fn name(&self) -> String {
        "Mock encoder".into()
    }
    fn codec(&self) -> Codec {
        self.codec
    }
}

struct MockEncoders;

impl EncoderFactory for MockEncoders {
    fn create(&self, req: &EncoderRequest) -> Result<Box<dyn VideoEncoder>, String> {
        Ok(Box::new(MockEncoder {
            codec: req.codec,
            frames: 0,
        }))
    }
    fn available(&self) -> Vec<(Codec, String, bool)> {
        vec![(Codec::H264, "Mock encoder".into(), false)]
    }
}

pub struct TestEnv {
    pub elevation: Arc<MockElevationState>,
    pub env: PlatformEnv,
    pub input: InputLog,
    pub clipboard: Arc<Mutex<SharedClipboard>>,
}

pub fn test_env() -> TestEnv {
    let input: InputLog = Arc::default();
    let clipboard: Arc<Mutex<SharedClipboard>> = Arc::default();
    let (i2, c2) = (input.clone(), clipboard.clone());
    let elevation: Arc<MockElevationState> = Arc::default();
    let displays = vec![
        DisplayInfo {
            index: 0,
            name: "Display 1".into(),
            width: 640,
            height: 360,
            x: 0,
            y: 0,
            primary: true,
        },
        DisplayInfo {
            index: 1,
            name: "Display 2".into(),
            width: 320,
            height: 240,
            x: 640,
            y: 0,
            primary: false,
        },
    ];
    let env = PlatformEnv {
        screen: Arc::new(move || {
            Ok(Box::new(MockScreen {
                displays: displays.clone(),
                active: 0,
                counter: 0,
                forced: false,
            }))
        }),
        encoders: Arc::new(MockEncoders),
        input: Arc::new(move || Box::new(MockInput(i2.clone()))),
        clipboard: Arc::new(move |_| Some(Box::new(MockClipboard(c2.clone())) as BoxedClipboard)),
        local_addresses: Arc::new(|| vec![std::net::IpAddr::from([127, 0, 0, 1])]),
        elevation: Arc::new(MockElevation(elevation.clone())),
    };
    TestEnv {
        elevation,
        env,
        input,
        clipboard,
    }
}
