//! Desktop attachment and picture capture for the agent.
//!
//! Windows keeps the login screen and UAC prompts on a separate "secure" desktop. A process running as
//! SYSTEM may attach its thread to whichever desktop currently receives input, and from there capture
//! it with GDI and inject input into it. Nothing here bypasses UAC: the consent prompt still has to be
//! answered, it can just be *seen and answered* by a session the owner explicitly trusted.

use remotex_common::peer::DisplayInfo;
use remotex_elevate::{nv12::bgra_to_nv12, Picture, MAX_DIMENSION};
use std::io;
use windows::Win32::{
    Foundation::HANDLE,
    Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
        ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HGDIOBJ,
        SRCCOPY,
    },
    System::StationsAndDesktops::{
        CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, SetThreadDesktop, DESKTOP_ACCESS_FLAGS,
        DESKTOP_CONTROL_FLAGS, HDESK, UOI_NAME,
    },
    UI::WindowsAndMessaging::{GetCursorInfo, CURSORINFO, CURSOR_SHOWING},
};

fn err(e: windows::core::Error) -> io::Error {
    io::Error::other(e.to_string())
}

const GENERIC_ALL_ACCESS: u32 = 0x1000_0000;

/// Keeps the calling thread attached to the desktop that currently receives input.
#[derive(Default)]
pub struct DesktopAttachment {
    current: Option<HDESK>,
    name: String,
}

impl DesktopAttachment {
    /// Re-attaches if the input desktop changed (a UAC prompt appeared or went away). Returns its name.
    pub fn follow_input_desktop(&mut self) -> io::Result<&str> {
        // SAFETY: plain desktop API calls; the previous handle is closed only after the switch.
        unsafe {
            let d = OpenInputDesktop(
                DESKTOP_CONTROL_FLAGS(0),
                false,
                DESKTOP_ACCESS_FLAGS(GENERIC_ALL_ACCESS),
            )
            .map_err(err)?;
            let name = desktop_name(d);
            if self.current.is_none() || name != self.name {
                SetThreadDesktop(d).map_err(err)?;
                if let Some(old) = self.current.replace(d) {
                    let _ = CloseDesktop(old);
                }
                self.name = name;
            } else {
                let _ = CloseDesktop(d);
            }
        }
        Ok(&self.name)
    }

    /// True while a desktop other than the ordinary user desktop is in front (UAC prompt, lock screen).
    pub fn is_secure(&self) -> bool {
        !self.name.is_empty() && !self.name.eq_ignore_ascii_case("Default")
    }
}

unsafe fn desktop_name(d: HDESK) -> String {
    let mut buf = vec![0u16; 256];
    let mut needed = 0u32;
    if GetUserObjectInformationW(
        HANDLE(d.0),
        UOI_NAME,
        Some(buf.as_mut_ptr().cast()),
        (buf.len() * 2) as u32,
        Some(&mut needed),
    )
    .is_err()
    {
        return String::new();
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    String::from_utf16_lossy(&buf[..len])
}

/// Captures `display` from the desktop the calling thread is attached to and scales it to NV12.
pub fn capture(display: &DisplayInfo, out_w: u32, out_h: u32) -> io::Result<Picture> {
    let (w, h) = (display.width, display.height);
    if w == 0 || h == 0 || w > MAX_DIMENSION || h > MAX_DIMENSION {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad display size"));
    }
    // SAFETY: standard GDI screen capture; every object created here is released before returning.
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(io::Error::last_os_error());
        }
        let dc = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w as i32, h as i32);
        let old = SelectObject(dc, HGDIOBJ(bmp.0));
        let copied = BitBlt(
            dc,
            0,
            0,
            w as i32,
            h as i32,
            Some(screen),
            display.x,
            display.y,
            SRCCOPY | CAPTUREBLT,
        );
        let mut bgra = vec![0u8; w as usize * h as usize * 4];
        let mut bmi = BITMAPINFO {
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
        let lines = GetDIBits(
            dc,
            bmp,
            0,
            h,
            Some(bgra.as_mut_ptr().cast()),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        SelectObject(dc, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        if copied.is_err() || lines == 0 {
            return Err(io::Error::other("screen copy failed"));
        }
        let nv12 = bgra_to_nv12(&bgra, w as usize, h as usize, out_w as usize, out_h as usize)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad output size"))?;
        Ok(Picture {
            width: out_w,
            height: out_h,
            nv12,
            pointer: pointer(),
        })
    }
}

fn pointer() -> Option<(i32, i32, bool)> {
    let mut ci = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: cbSize is set as the API requires.
    unsafe { GetCursorInfo(&mut ci) }.ok()?;
    Some((
        ci.ptScreenPos.x,
        ci.ptScreenPos.y,
        ci.flags.0 & CURSOR_SHOWING.0 != 0,
    ))
}
