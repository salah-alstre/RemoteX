//! Mouse cursor shape capture. Desktop Duplication delivers the desktop without the cursor, so the
//! shape is sent separately and drawn by the viewer.

use windows::Win32::{
    Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetObjectW, ReleaseDC,
        SelectObject, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
    },
    UI::WindowsAndMessaging::{
        DrawIconEx, GetCursorInfo, GetIconInfo, CURSORINFO, CURSOR_SHOWING, DI_NORMAL, HCURSOR, HICON,
        ICONINFO,
    },
};

pub struct CursorImage {
    pub id: u32,
    pub width: u16,
    pub height: u16,
    pub hot_x: u16,
    pub hot_y: u16,
    /// Straight-alpha RGBA.
    pub rgba: Vec<u8>,
}

#[derive(Default)]
pub struct CursorTracker {
    last: Option<isize>,
    next_id: u32,
}

impl CursorTracker {
    /// Returns a new image only when the system cursor shape changed since the previous call.
    pub fn poll(&mut self) -> Option<CursorImage> {
        // SAFETY: GetCursorInfo fills a properly sized struct; handles are only read.
        unsafe {
            let mut ci = CURSORINFO {
                cbSize: std::mem::size_of::<CURSORINFO>() as u32,
                ..Default::default()
            };
            GetCursorInfo(&mut ci).ok()?;
            if ci.flags.0 & CURSOR_SHOWING.0 == 0 || ci.hCursor.is_invalid() {
                return None;
            }
            let handle = ci.hCursor.0 as isize;
            if self.last == Some(handle) {
                return None;
            }
            let img = render(ci.hCursor, self.next_id)?;
            self.last = Some(handle);
            self.next_id += 1;
            Some(img)
        }
    }

    /// Forces the current shape to be resent (new viewer joined).
    pub fn reset(&mut self) {
        self.last = None;
    }
}

/// Renders the cursor twice (on black and on white) and recovers per-pixel alpha from the difference.
/// This handles alpha cursors, classic masked cursors and XOR cursors uniformly.
unsafe fn render(cursor: HCURSOR, id: u32) -> Option<CursorImage> {
    let mut info = ICONINFO::default();
    GetIconInfo(cursor.into(), &mut info).ok()?;
    let mut bm = BITMAP::default();
    let probe: HBITMAP = if info.hbmColor.is_invalid() {
        info.hbmMask
    } else {
        info.hbmColor
    };
    GetObjectW(
        HGDIOBJ(probe.0),
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut bm as *mut _ as *mut _),
    );
    let mono = info.hbmColor.is_invalid();
    let (w, h) = (
        bm.bmWidth.max(0) as usize,
        if mono {
            (bm.bmHeight / 2).max(0)
        } else {
            bm.bmHeight.max(0)
        } as usize,
    );
    let (hot_x, hot_y) = (info.xHotspot as u16, info.yHotspot as u16);
    let cleanup = |info: &ICONINFO| {
        if !info.hbmMask.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
        }
        if !info.hbmColor.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
        }
    };
    if w == 0 || h == 0 || w > 128 || h > 128 {
        cleanup(&info);
        return None;
    }

    let screen = GetDC(None);
    let dc = CreateCompatibleDC(Some(screen));
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
    let result = match dib {
        Ok(dib) if !bits.is_null() => {
            let old = SelectObject(dc, HGDIOBJ(dib.0));
            let px = std::slice::from_raw_parts_mut(bits as *mut u8, w * h * 4);
            let mut draw = |fill: u8| -> Vec<u8> {
                px.fill(fill);
                let _ = DrawIconEx(dc, 0, 0, HICON(cursor.0), w as i32, h as i32, 0, None, DI_NORMAL);
                px.to_vec()
            };
            let on_black = draw(0);
            let on_white = draw(255);
            let mut rgba = vec![0u8; w * h * 4];
            for i in 0..w * h {
                let o = i * 4;
                // Per channel: white_bg = c*a + 255*(1-a); black_bg = c*a  =>  a = 1 - (white - black)/255.
                let diff = (0..3)
                    .map(|c| on_white[o + c].saturating_sub(on_black[o + c]) as u32)
                    .max()
                    .unwrap_or(0);
                let a = 255 - diff.min(255);
                if a > 0 {
                    for c in 0..3 {
                        // DIB is BGRA; un-premultiply into RGBA.
                        let v = (on_black[o + c] as u32 * 255)
                            .checked_div(a)
                            .unwrap_or(0)
                            .min(255) as u8;
                        rgba[o + 2 - c] = v;
                    }
                    rgba[o + 3] = a as u8;
                }
            }
            SelectObject(dc, old);
            let _ = DeleteObject(HGDIOBJ(dib.0));
            Some(CursorImage {
                id,
                width: w as u16,
                height: h as u16,
                hot_x,
                hot_y,
                rgba,
            })
        }
        _ => None,
    };
    let _ = DeleteDC(dc);
    ReleaseDC(None, screen);
    cleanup(&info);
    result
}
