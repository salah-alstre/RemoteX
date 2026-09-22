//! Keyboard and mouse injection with `SendInput`.
//!
//! Keys travel as physical scancodes so the *host's* active layout decides the produced character;
//! this keeps English and Arabic layouts correct on either side. Text that has no physical key is
//! injected as Unicode. The injector remembers what is held so nothing can stay stuck.

use remotex_common::peer::{DisplayInfo, InputEvent, MouseButton};
use std::collections::HashSet;
use windows::Win32::UI::{
    Input::KeyboardAndMouse::*,
    WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtualScreen {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl VirtualScreen {
    pub fn current() -> Self {
        // SAFETY: GetSystemMetrics has no preconditions.
        unsafe {
            Self {
                x: GetSystemMetrics(SM_XVIRTUALSCREEN),
                y: GetSystemMetrics(SM_YVIRTUALSCREEN),
                w: GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
                h: GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
            }
        }
    }
}

/// Maps a position normalised over `display` (0..=65535) to SendInput's virtual-desktop range.
pub fn to_virtual_abs(nx: u16, ny: u16, display: &DisplayInfo, v: VirtualScreen) -> (i32, i32) {
    let px = display.x as f64 + nx as f64 / 65535.0 * (display.width.saturating_sub(1)) as f64;
    let py = display.y as f64 + ny as f64 / 65535.0 * (display.height.saturating_sub(1)) as f64;
    let ax = ((px - v.x as f64) * 65535.0 / (v.w - 1).max(1) as f64).round() as i32;
    let ay = ((py - v.y as f64) * 65535.0 / (v.h - 1).max(1) as f64).round() as i32;
    (ax.clamp(0, 65535), ay.clamp(0, 65535))
}

fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn key(scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn button_flags(b: MouseButton, down: bool) -> (MOUSE_EVENT_FLAGS, u32) {
    match (b, down) {
        (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (MouseButton::Back, true) => (MOUSEEVENTF_XDOWN, 1),
        (MouseButton::Back, false) => (MOUSEEVENTF_XUP, 1),
        (MouseButton::Forward, true) => (MOUSEEVENTF_XDOWN, 2),
        (MouseButton::Forward, false) => (MOUSEEVENTF_XUP, 2),
    }
}

#[derive(Default)]
pub struct Injector {
    held_keys: HashSet<(u16, bool)>,
    held_buttons: HashSet<u8>,
}

fn button_id(b: MouseButton) -> u8 {
    b as u8
}

impl Injector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the SendInput records for an event and updates held-state bookkeeping.
    pub fn build(&mut self, ev: &InputEvent, display: &DisplayInfo, v: VirtualScreen) -> Vec<INPUT> {
        match ev {
            InputEvent::MouseMove { x, y } => {
                let (ax, ay) = to_virtual_abs(*x, *y, display, v);
                vec![mouse(
                    MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    ax,
                    ay,
                    0,
                )]
            }
            InputEvent::MouseButton { button, down } => {
                if *down {
                    self.held_buttons.insert(button_id(*button));
                } else {
                    self.held_buttons.remove(&button_id(*button));
                }
                let (f, data) = button_flags(*button, *down);
                vec![mouse(f, 0, 0, data)]
            }
            InputEvent::Wheel { dx, dy } => {
                let mut out = Vec::new();
                if *dy != 0 {
                    out.push(mouse(MOUSEEVENTF_WHEEL, 0, 0, *dy as i32 as u32));
                }
                if *dx != 0 {
                    out.push(mouse(MOUSEEVENTF_HWHEEL, 0, 0, *dx as i32 as u32));
                }
                out
            }
            InputEvent::Key {
                scancode,
                extended,
                down,
            } => {
                if *scancode == 0 || *scancode > 0xff {
                    return Vec::new();
                }
                let id = (*scancode, *extended);
                if *down {
                    self.held_keys.insert(id);
                } else {
                    self.held_keys.remove(&id);
                }
                let mut f = KEYEVENTF_SCANCODE;
                if *extended {
                    f |= KEYEVENTF_EXTENDEDKEY;
                }
                if !*down {
                    f |= KEYEVENTF_KEYUP;
                }
                vec![key(*scancode, f)]
            }
            InputEvent::Text(text) => text
                .encode_utf16()
                .flat_map(|u| {
                    [
                        key(u, KEYEVENTF_UNICODE),
                        key(u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
                    ]
                })
                .collect(),
            InputEvent::ReleaseAll => self.release_records(),
        }
    }

    fn release_records(&mut self) -> Vec<INPUT> {
        let mut out = Vec::new();
        for (scan, ext) in self.held_keys.drain() {
            let mut f = KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP;
            if ext {
                f |= KEYEVENTF_EXTENDEDKEY;
            }
            out.push(key(scan, f));
        }
        let buttons: Vec<u8> = self.held_buttons.drain().collect();
        for b in buttons {
            let btn = match b {
                x if x == MouseButton::Left as u8 => MouseButton::Left,
                x if x == MouseButton::Right as u8 => MouseButton::Right,
                x if x == MouseButton::Middle as u8 => MouseButton::Middle,
                x if x == MouseButton::Back as u8 => MouseButton::Back,
                _ => MouseButton::Forward,
            };
            let (f, data) = button_flags(btn, false);
            out.push(mouse(f, 0, 0, data));
        }
        out
    }

    pub fn apply(&mut self, ev: &InputEvent, display: &DisplayInfo) {
        let records = self.build(ev, display, VirtualScreen::current());
        send(&records);
    }

    /// Releases every held key and button. Called on viewer blur, permission loss and disconnect.
    pub fn release_all(&mut self) {
        let records = self.release_records();
        send(&records);
    }

    pub fn held_count(&self) -> usize {
        self.held_keys.len() + self.held_buttons.len()
    }
}

fn send(records: &[INPUT]) {
    if records.is_empty() {
        return;
    }
    // SAFETY: `records` is a valid slice of fully initialised INPUT structs.
    let sent = unsafe { SendInput(records, std::mem::size_of::<INPUT>() as i32) };
    if (sent as usize) < records.len() {
        tracing::debug!(
            sent,
            expected = records.len(),
            "SendInput injected fewer events than requested (blocked by UIPI or secure desktop)"
        );
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        self.release_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disp(x: i32, y: i32, w: u32, h: u32) -> DisplayInfo {
        DisplayInfo {
            index: 0,
            name: "d".into(),
            width: w,
            height: h,
            x,
            y,
            primary: x == 0,
        }
    }
    const V: VirtualScreen = VirtualScreen {
        x: 0,
        y: 0,
        w: 3840,
        h: 1080,
    };

    #[test]
    fn maps_second_monitor_into_virtual_space() {
        let d2 = disp(1920, 0, 1920, 1080);
        assert_eq!(
            to_virtual_abs(0, 0, &d2, V).0,
            (1920.0f64 * 65535.0 / 3839.0).round() as i32
        );
        assert_eq!(to_virtual_abs(65535, 65535, &d2, V), (65535, 65535));
        let d1 = disp(0, 0, 1920, 1080);
        assert_eq!(to_virtual_abs(0, 0, &d1, V), (0, 0));
    }

    #[test]
    fn negative_origin_monitors_are_supported() {
        let v = VirtualScreen {
            x: -1920,
            y: 0,
            w: 3840,
            h: 1080,
        };
        let left = disp(-1920, 0, 1920, 1080);
        assert_eq!(to_virtual_abs(0, 0, &left, v), (0, 0));
    }

    #[test]
    fn key_state_is_tracked_and_released() {
        let mut inj = Injector::new();
        let d = disp(0, 0, 1920, 1080);
        inj.build(
            &InputEvent::Key {
                scancode: 0x1d,
                extended: false,
                down: true,
            },
            &d,
            V,
        );
        inj.build(
            &InputEvent::MouseButton {
                button: MouseButton::Left,
                down: true,
            },
            &d,
            V,
        );
        assert_eq!(inj.held_count(), 2);
        let rel = inj.build(&InputEvent::ReleaseAll, &d, V);
        assert_eq!(rel.len(), 2);
        assert_eq!(inj.held_count(), 0);
        // key up removes the entry without leaving a phantom hold
        inj.build(
            &InputEvent::Key {
                scancode: 0x1e,
                extended: true,
                down: true,
            },
            &d,
            V,
        );
        inj.build(
            &InputEvent::Key {
                scancode: 0x1e,
                extended: true,
                down: false,
            },
            &d,
            V,
        );
        assert_eq!(inj.held_count(), 0);
    }

    #[test]
    fn unicode_text_produces_down_up_pairs_including_surrogates() {
        let mut inj = Injector::new();
        let d = disp(0, 0, 1920, 1080);
        assert_eq!(inj.build(&InputEvent::Text("مرحبا".into()), &d, V).len(), 10);
        assert_eq!(inj.build(&InputEvent::Text("😀".into()), &d, V).len(), 4);
    }

    #[test]
    fn invalid_scancodes_are_ignored() {
        let mut inj = Injector::new();
        let d = disp(0, 0, 1920, 1080);
        assert!(inj
            .build(
                &InputEvent::Key {
                    scancode: 0,
                    extended: false,
                    down: true
                },
                &d,
                V
            )
            .is_empty());
        assert!(inj
            .build(
                &InputEvent::Key {
                    scancode: 0x1ff,
                    extended: false,
                    down: true
                },
                &d,
                V
            )
            .is_empty());
    }
}
