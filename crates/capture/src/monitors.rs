use crate::CaptureError;
use windows::{
    core::Interface,
    Win32::Graphics::{
        Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput},
        Gdi::{GetMonitorInfoW, MONITORINFO},
    },
};

#[derive(Debug, Clone)]
pub struct Monitor {
    pub index: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub primary: bool,
    pub(crate) adapter: u32,
    pub(crate) output: u32,
}

pub fn list_monitors() -> Result<Vec<Monitor>, CaptureError> {
    // SAFETY: DXGI enumeration; all returned interfaces are reference counted by the bindings.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut out = Vec::new();
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                let desc = output.GetDesc()?;
                if desc.AttachedToDesktop.as_bool() {
                    let r = desc.DesktopCoordinates;
                    let mut info = MONITORINFO {
                        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    let primary =
                        GetMonitorInfoW(desc.Monitor, &mut info).as_bool() && (info.dwFlags & 1) != 0;
                    let len = desc
                        .DeviceName
                        .iter()
                        .position(|c| *c == 0)
                        .unwrap_or(desc.DeviceName.len());
                    let raw = String::from_utf16_lossy(&desc.DeviceName[..len]);
                    let index = out.len() as u32;
                    out.push(Monitor {
                        index,
                        name: raw.trim_start_matches("\\\\.\\").to_string(),
                        width: (r.right - r.left) as u32,
                        height: (r.bottom - r.top) as u32,
                        x: r.left,
                        y: r.top,
                        primary,
                        adapter: a,
                        output: o,
                    });
                }
                o += 1;
            }
            a += 1;
        }
        // Primary first so index 0 is what users expect as "Display 1".
        out.sort_by_key(|m| (!m.primary, m.x, m.y));
        for (i, m) in out.iter_mut().enumerate() {
            m.index = i as u32;
        }
        Ok(out)
    }
}

pub(crate) fn open_output(
    m: &Monitor,
) -> Result<(windows::Win32::Graphics::Dxgi::IDXGIAdapter1, IDXGIOutput), CaptureError> {
    // SAFETY: as above.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let adapter = factory.EnumAdapters1(m.adapter)?;
        let output = adapter.EnumOutputs(m.output)?;
        let _ = output.cast::<IDXGIOutput>();
        Ok((adapter, output))
    }
}
