//! GPU BGRA -> NV12 conversion and scaling using the D3D11 video processor.

use crate::{CaptureError, Nv12Frame};
use std::mem::ManuallyDrop;
use windows::{
    core::Interface,
    Win32::Graphics::{
        Direct3D11::*,
        Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC},
    },
};

pub struct Converter {
    video_ctx: ID3D11VideoContext,
    video_dev: ID3D11VideoDevice,
    ctx: ID3D11DeviceContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    out_view: ID3D11VideoProcessorOutputView,
    nv12: ID3D11Texture2D,
    staging: ID3D11Texture2D,
    pub out_w: u32,
    pub out_h: u32,
    src_w: u32,
    src_h: u32,
}

// Bit layout of D3D11_VIDEO_PROCESSOR_COLOR_SPACE: usage:1 rgb_range:1 matrix:1 xvycc:1 nominal_range:2.
const CS_RGB_FULL: u32 = 0;
const CS_YCBCR_709_LIMITED: u32 = (1 << 2) | (1 << 4);

impl Converter {
    pub fn new(
        device: &ID3D11Device,
        ctx: &ID3D11DeviceContext,
        src_w: u32,
        src_h: u32,
        out_w: u32,
        out_h: u32,
    ) -> Result<Self, CaptureError> {
        // NV12 requires even dimensions.
        let (out_w, out_h) = (out_w & !1, out_h & !1);
        // SAFETY: D3D11 resource creation with fully initialised descriptors.
        unsafe {
            let video_dev: ID3D11VideoDevice = device.cast()?;
            let video_ctx: ID3D11VideoContext = ctx.cast()?;
            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputWidth: src_w,
                InputHeight: src_h,
                OutputWidth: out_w,
                OutputHeight: out_h,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
                ..Default::default()
            };
            let enumerator = video_dev.CreateVideoProcessorEnumerator(&desc)?;
            let processor = video_dev.CreateVideoProcessor(&enumerator, 0)?;

            let mut tex_desc = D3D11_TEXTURE2D_DESC {
                Width: out_w,
                Height: out_h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                ..Default::default()
            };
            let mut nv12 = None;
            device.CreateTexture2D(&tex_desc, None, Some(&mut nv12))?;
            let nv12 = nv12.expect("texture created");

            tex_desc.Usage = D3D11_USAGE_STAGING;
            tex_desc.BindFlags = 0;
            tex_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            device.CreateTexture2D(&tex_desc, None, Some(&mut staging))?;
            let staging = staging.expect("texture created");

            let view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                ..Default::default()
            };
            let mut out_view = None;
            video_dev.CreateVideoProcessorOutputView(&nv12, &enumerator, &view_desc, Some(&mut out_view))?;
            let out_view = out_view.expect("view created");

            let output_space = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                _bitfield: CS_YCBCR_709_LIMITED,
            };
            video_ctx.VideoProcessorSetOutputColorSpace(&processor, &output_space);
            let input_space = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                _bitfield: CS_RGB_FULL,
            };
            video_ctx.VideoProcessorSetStreamColorSpace(&processor, 0, &input_space);
            video_ctx.VideoProcessorSetStreamOutputRate(
                &processor,
                0,
                D3D11_VIDEO_PROCESSOR_OUTPUT_RATE_NORMAL,
                true,
                None,
            );

            Ok(Self {
                video_ctx,
                video_dev,
                ctx: ctx.clone(),
                enumerator,
                processor,
                out_view,
                nv12,
                staging,
                out_w,
                out_h,
                src_w,
                src_h,
            })
        }
    }

    pub fn matches(&self, src_w: u32, src_h: u32, out_w: u32, out_h: u32) -> bool {
        self.src_w == src_w && self.src_h == src_h && self.out_w == (out_w & !1) && self.out_h == (out_h & !1)
    }

    /// Converts `source` (BGRA desktop image) and returns a packed NV12 buffer.
    pub fn convert(&mut self, source: &ID3D11Texture2D) -> Result<Nv12Frame, CaptureError> {
        // SAFETY: the views/textures are owned by `self`; `Map` results are only read while mapped.
        unsafe {
            let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV {
                        MipSlice: 0,
                        ArraySlice: 0,
                    },
                },
                ..Default::default()
            };
            let mut in_view = None;
            self.video_dev.CreateVideoProcessorInputView(
                source,
                &self.enumerator,
                &in_desc,
                Some(&mut in_view),
            )?;
            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: ManuallyDrop::new(in_view),
                ..Default::default()
            };
            let streams = [stream];
            let blt = self
                .video_ctx
                .VideoProcessorBlt(&self.processor, &self.out_view, 0, &streams);
            // Release the input view reference held by the stream regardless of the result.
            let [stream] = streams;
            drop(ManuallyDrop::into_inner(stream.pInputSurface));
            blt?;

            self.ctx.CopyResource(&self.staging, &self.nv12);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let pitch = mapped.RowPitch as usize;
            let (w, h) = (self.out_w as usize, self.out_h as usize);
            let base = mapped.pData as *const u8;
            let mut data = Vec::with_capacity(w * h * 3 / 2);
            for row in 0..h {
                data.extend_from_slice(std::slice::from_raw_parts(base.add(row * pitch), w));
            }
            let uv = base.add(pitch * h);
            for row in 0..h / 2 {
                data.extend_from_slice(std::slice::from_raw_parts(uv.add(row * pitch), w));
            }
            self.ctx.Unmap(&self.staging, 0);
            Ok(Nv12Frame {
                width: self.out_w,
                height: self.out_h,
                data,
            })
        }
    }
}
