//! Platform abstraction. The engine only talks to these traits; `platform` provides the Windows
//! implementations and tests provide in-memory ones so the whole protocol runs without a desktop.

use remotex_common::peer::{Codec, CursorShape, DisplayInfo, InputEvent};
use remotex_input::ClipboardBackend;
use std::sync::Arc;

/// NV12 frame: `width*height` luma bytes followed by `width*height/2` interleaved chroma.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pointer {
    pub x: i32,
    pub y: i32,
    pub visible: bool,
}

#[derive(Default)]
pub struct SourceOutput {
    /// `None` when the desktop is unchanged.
    pub frame: Option<Frame>,
    pub pointer: Option<Pointer>,
    pub cursor_shape: Option<CursorShape>,
    pub convert_us: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// Recoverable: recreate the source (desktop switch, resolution change).
    #[error("desktop access lost")]
    Lost,
    #[error("{0}")]
    Fatal(String),
}

pub trait ScreenSource: Send {
    fn displays(&mut self) -> Vec<DisplayInfo>;
    fn select(&mut self, index: u32) -> Result<DisplayInfo, String>;
    fn next(&mut self, out_w: u32, out_h: u32, timeout_ms: u32) -> Result<SourceOutput, SourceError>;
    /// Guarantees the next `next` call yields a frame even if the desktop did not change.
    fn request_frame(&mut self);
}

#[derive(Debug, Clone)]
pub struct EncoderRequest {
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub hardware: bool,
}

pub struct EncodedUnit {
    pub data: Vec<u8>,
    pub keyframe: bool,
}

pub trait VideoEncoder: Send {
    fn encode(&mut self, frame: &Frame, force_key: bool) -> Result<Vec<EncodedUnit>, String>;
    fn set_bitrate(&mut self, kbps: u32);
    fn name(&self) -> String;
    fn codec(&self) -> Codec;
}

pub trait EncoderFactory: Send + Sync {
    fn create(&self, req: &EncoderRequest) -> Result<Box<dyn VideoEncoder>, String>;
    /// `(codec, name, is_hardware)` for diagnostics and the settings page.
    fn available(&self) -> Vec<(Codec, String, bool)>;
}

pub trait InputSink: Send {
    fn apply(&mut self, event: &InputEvent, display: &DisplayInfo);
    fn release_all(&mut self);
}

pub type BoxedClipboard = Box<dyn ClipboardBackend + Send>;

/// Input injection with administrator rights, through the RemoteX service.
pub trait ElevatedInput: Send {
    fn apply(&mut self, event: &InputEvent, display: &DisplayInfo);
    /// False once the service ended the grant (or became unreachable).
    fn alive(&self) -> bool;
}

pub struct ElevatedPicture {
    pub frame: Frame,
    pub pointer: Option<Pointer>,
}

/// Picture source for the secure desktop (UAC prompts), where ordinary capture cannot see.
pub trait ElevatedVideo: Send {
    /// True while a UAC prompt or other secure desktop is in front.
    fn secure_desktop(&mut self) -> bool;
    fn capture(&mut self, display: &DisplayInfo, out_w: u32, out_h: u32) -> Option<ElevatedPicture>;
}

/// One approved elevated session. Dropping `guard` revokes it at the service.
pub struct ElevatedSession {
    pub input: Box<dyn ElevatedInput>,
    pub video: Box<dyn ElevatedVideo>,
    pub guard: Box<dyn Send>,
}

pub trait ElevationProvider: Send + Sync {
    /// Whether the elevated-control service is installed and answering.
    fn available(&self) -> bool;
    /// Starts elevated control for the session the owner of this PC just approved.
    fn begin(&self, label: &str) -> Result<ElevatedSession, String>;
}

/// Everything platform-specific the engine needs.
#[derive(Clone)]
pub struct PlatformEnv {
    pub screen: Arc<dyn Fn() -> Result<Box<dyn ScreenSource>, String> + Send + Sync>,
    pub encoders: Arc<dyn EncoderFactory>,
    pub input: Arc<dyn Fn() -> Box<dyn InputSink> + Send + Sync>,
    pub clipboard: Arc<dyn Fn(bool) -> Option<BoxedClipboard> + Send + Sync>,
    pub local_addresses: Arc<dyn Fn() -> Vec<std::net::IpAddr> + Send + Sync>,
    pub elevation: Arc<dyn ElevationProvider>,
}
