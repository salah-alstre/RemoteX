use crate::ClipboardBackend;
use arboard::{Clipboard, ImageData};
use remotex_common::peer::ClipboardPayload;
use std::borrow::Cow;
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;

/// Windows clipboard via `arboard`; change detection uses the system sequence number.
pub struct SystemClipboard {
    inner: Clipboard,
    images: bool,
}

impl SystemClipboard {
    pub fn new(images: bool) -> Result<Self, String> {
        Ok(Self {
            inner: Clipboard::new().map_err(|e| e.to_string())?,
            images,
        })
    }
}

fn encode_png(img: &ImageData) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, img.width as u32, img.height as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let mut w = enc.write_header().ok()?;
    w.write_image_data(&img.bytes).ok()?;
    w.finish().ok()?;
    Some(out)
}

fn decode_png(bytes: &[u8]) -> Option<ImageData<'static>> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().ok()?;
    if reader.info().width > 8192 || reader.info().height > 8192 {
        return None;
    }
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        _ => return None,
    };
    Some(ImageData {
        width: info.width as usize,
        height: info.height as usize,
        bytes: Cow::Owned(rgba),
    })
}

impl ClipboardBackend for SystemClipboard {
    fn sequence(&self) -> u32 {
        // SAFETY: no arguments; returns a plain counter.
        unsafe { GetClipboardSequenceNumber() }
    }

    fn read(&mut self) -> Option<ClipboardPayload> {
        if let Ok(t) = self.inner.get_text() {
            return Some(ClipboardPayload::Text(t));
        }
        if self.images {
            let img = self.inner.get_image().ok()?;
            return encode_png(&img).map(ClipboardPayload::Image);
        }
        None
    }

    fn write(&mut self, payload: &ClipboardPayload) -> Result<(), String> {
        match payload {
            ClipboardPayload::Text(t) => self.inner.set_text(t.clone()).map_err(|e| e.to_string()),
            ClipboardPayload::Image(png) if self.images => {
                let img = decode_png(png).ok_or("unsupported image")?;
                self.inner.set_image(img).map_err(|e| e.to_string())
            }
            ClipboardPayload::Image(_) => Err("image clipboard disabled".into()),
        }
    }
}
