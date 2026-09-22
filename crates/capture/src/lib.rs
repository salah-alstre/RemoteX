//! Windows screen capture: DXGI Desktop Duplication with GPU-side scaling and NV12 conversion.
//!
//! The desktop image never leaves the GPU until it has been scaled and converted to NV12 by the
//! D3D11 video processor; only that (3 bytes per 2 pixels) is read back for the encoder.

#![cfg(windows)]

mod converter;
mod cursor;
mod duplication;
mod monitors;

pub use cursor::{CursorImage, CursorTracker};
pub use duplication::{Captured, Capturer, PointerState};
pub use monitors::{list_monitors, Monitor};

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// The desktop switched (UAC, lock screen, resolution change); recreate the capturer.
    #[error("desktop access lost")]
    AccessLost,
    #[error("monitor {0} not found")]
    NoSuchMonitor(u32),
    #[error("windows api error: {0}")]
    Windows(#[from] windows::core::Error),
}

/// Planar-interleaved YUV 4:2:0 frame: `width*height` luma bytes followed by `width*height/2` chroma.
#[derive(Clone)]
pub struct Nv12Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Makes the process per-monitor DPI aware so coordinates are physical pixels.
pub fn enable_dpi_awareness() {
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    // SAFETY: plain FFI call with a valid constant; failure just means it was already set.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}
