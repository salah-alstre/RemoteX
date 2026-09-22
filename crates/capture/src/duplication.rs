use crate::{converter::Converter, monitors, CaptureError, Monitor, Nv12Frame};
use windows::{
    core::Interface,
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0},
            Direct3D11::*,
            Dxgi::*,
        },
    },
};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PointerState {
    pub visible: bool,
    /// Position in monitor pixels.
    pub x: i32,
    pub y: i32,
}

pub struct Captured {
    /// `None` when the desktop image did not change since the previous call.
    pub frame: Option<Nv12Frame>,
    pub pointer: PointerState,
    /// GPU time spent converting plus readback, microseconds.
    pub convert_us: u64,
}

pub struct Capturer {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    monitor: Monitor,
    src_w: u32,
    src_h: u32,
    conv: Option<Converter>,
    pointer: PointerState,
    /// A frame is owed to the caller even if the desktop is static (first frame / after keyframe request).
    force_next: bool,
    last_texture: Option<ID3D11Texture2D>,
}

impl Capturer {
    pub fn new(monitor: &Monitor) -> Result<Self, CaptureError> {
        let (adapter, output) = monitors::open_output(monitor)?;
        // SAFETY: standard D3D11/DXGI initialisation; interfaces are ref counted.
        unsafe {
            let mut device = None;
            let mut ctx = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )?;
            let (device, ctx) = (device.expect("device"), ctx.expect("context"));
            let output1: IDXGIOutput1 = output.cast()?;
            let dup = output1.DuplicateOutput(&device).map_err(map_dxgi)?;
            let desc = dup.GetDesc();
            Ok(Self {
                device,
                ctx,
                dup,
                monitor: monitor.clone(),
                src_w: desc.ModeDesc.Width,
                src_h: desc.ModeDesc.Height,
                conv: None,
                pointer: PointerState::default(),
                force_next: true,
                last_texture: None,
            })
        }
    }

    pub fn monitor(&self) -> &Monitor {
        &self.monitor
    }

    /// Ensures the next call returns a frame even if the desktop has not changed.
    pub fn request_frame(&mut self) {
        self.force_next = true;
    }

    /// Waits up to `timeout_ms` for a desktop change and returns it scaled to `out_w` x `out_h`.
    pub fn next(&mut self, out_w: u32, out_h: u32, timeout_ms: u32) -> Result<Captured, CaptureError> {
        // SAFETY: AcquireNextFrame/ReleaseFrame are paired on every path below.
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            match self.dup.AcquireNextFrame(timeout_ms, &mut info, &mut resource) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return self.idle(out_w, out_h),
                Err(e) => return Err(map_dxgi(e)),
            }
            if info.LastMouseUpdateTime != 0 {
                self.pointer = PointerState {
                    visible: info.PointerPosition.Visible.as_bool(),
                    x: info.PointerPosition.Position.x,
                    y: info.PointerPosition.Position.y,
                };
            }
            let changed = info.LastPresentTime != 0;
            let start = std::time::Instant::now();
            let result = if changed {
                let tex: ID3D11Texture2D = resource.as_ref().expect("resource").cast()?;
                self.convert(&tex, out_w, out_h).map(Some)
            } else {
                Ok(None)
            };
            let _ = self.dup.ReleaseFrame();
            let frame = match result? {
                Some(f) => Some(f),
                None if self.force_next => self.reconvert(out_w, out_h)?,
                None => None,
            };
            if frame.is_some() {
                self.force_next = false;
            }
            Ok(Captured {
                frame,
                pointer: self.pointer,
                convert_us: start.elapsed().as_micros() as u64,
            })
        }
    }

    fn idle(&mut self, out_w: u32, out_h: u32) -> Result<Captured, CaptureError> {
        let frame = if self.force_next {
            self.reconvert(out_w, out_h)?
        } else {
            None
        };
        if frame.is_some() {
            self.force_next = false;
        }
        Ok(Captured {
            frame,
            pointer: self.pointer,
            convert_us: 0,
        })
    }

    /// Re-produces the current desktop for a forced refresh by copying the last converted source.
    fn reconvert(&mut self, out_w: u32, out_h: u32) -> Result<Option<Nv12Frame>, CaptureError> {
        // SAFETY: the copy texture is owned by self; sizes match the duplication description.
        unsafe {
            let Some(tex) = self.last_texture.clone() else {
                return Ok(None);
            };
            self.run_converter(&tex, out_w, out_h).map(Some)
        }
    }

    unsafe fn convert(
        &mut self,
        tex: &ID3D11Texture2D,
        out_w: u32,
        out_h: u32,
    ) -> Result<Nv12Frame, CaptureError> {
        // Keep a private copy of the desktop so forced refreshes work on a static screen.
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_DEFAULT;
        desc.BindFlags = (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32;
        desc.CPUAccessFlags = 0;
        desc.MiscFlags = 0;
        let keep = match &self.last_texture {
            Some(t) => t.clone(),
            None => {
                let mut t = None;
                self.device.CreateTexture2D(&desc, None, Some(&mut t))?;
                t.expect("texture")
            }
        };
        self.ctx.CopyResource(&keep, tex);
        self.last_texture = Some(keep.clone());
        self.run_converter(&keep, out_w, out_h)
    }

    unsafe fn run_converter(
        &mut self,
        keep: &ID3D11Texture2D,
        out_w: u32,
        out_h: u32,
    ) -> Result<Nv12Frame, CaptureError> {
        if !self
            .conv
            .as_ref()
            .is_some_and(|c| c.matches(self.src_w, self.src_h, out_w, out_h))
        {
            self.conv = Some(Converter::new(
                &self.device,
                &self.ctx,
                self.src_w,
                self.src_h,
                out_w,
                out_h,
            )?);
        }
        self.conv.as_mut().expect("converter").convert(keep)
    }
}

fn map_dxgi(e: windows::core::Error) -> CaptureError {
    if e.code() == DXGI_ERROR_ACCESS_LOST
        || e.code() == DXGI_ERROR_INVALID_CALL
        || e.code() == DXGI_ERROR_SESSION_DISCONNECTED
    {
        CaptureError::AccessLost
    } else {
        CaptureError::Windows(e)
    }
}
