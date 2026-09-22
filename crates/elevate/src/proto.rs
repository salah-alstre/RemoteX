//! Messages exchanged over the local pipes and their validation.

use remotex_common::peer::{DisplayInfo, InputEvent};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{Read, Write};

pub const PROTOCOL_VERSION: u16 = 1;
/// The pipe the desktop app connects to. Its access list only admits SYSTEM, administrators and the
/// interactive user; the service additionally verifies who is really on the other end.
pub const SERVICE_PIPE: &str = r"\\.\pipe\remotex-service";
pub const SERVICE_NAME: &str = "RemoteXService";
pub const SERVICE_DISPLAY_NAME: &str = "RemoteX Elevated Control Service";

/// Largest frame accepted on any pipe (fits a 4K NV12 picture with room to spare).
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;
/// Largest picture the agent will produce or the service will relay.
pub const MAX_DIMENSION: u32 = 7680;
/// Seconds without a heartbeat after which an elevated grant lapses on its own.
pub const GRANT_LAPSE_SECS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnRole {
    /// Carries input and control; never waits for a picture.
    Input,
    /// Carries picture requests and status.
    Video,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UiToService {
    Hello {
        version: u16,
    },
    /// The owner of this PC approved elevated control for the current session.
    Grant {
        label: String,
    },
    /// Join an existing grant from a second connection (input and video use separate pipes).
    Attach {
        token: [u8; 16],
        role: ConnRole,
    },
    Revoke,
    Heartbeat,
    Input {
        ev: InputEvent,
        display: DisplayInfo,
    },
    /// Ask for the current picture of `display`, scaled to `out_w` x `out_h`.
    Capture {
        display: DisplayInfo,
        out_w: u32,
        out_h: u32,
    },
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DenyReason {
    /// The caller is not the installed RemoteX application, or not the signed-in user.
    NotAuthorized,
    NoActiveSession,
    AgentUnavailable,
    AlreadyGranted,
    NotGranted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// NV12: `width*height` luma bytes then `width*height/2` interleaved chroma.
    pub nv12: Vec<u8>,
    /// Pointer position in virtual-screen pixels and whether it is visible.
    pub pointer: Option<(i32, i32, bool)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServiceToUi {
    Welcome {
        version: u16,
        service_version: String,
    },
    Granted {
        token: [u8; 16],
    },
    Attached,
    Denied(DenyReason),
    Revoked,
    Picture(Picture),
    /// The capture could not be produced right now.
    NoPicture,
    Status {
        secure_desktop: bool,
        agent_running: bool,
    },
}

/// Service <-> agent. The agent is a pure executor: it only does what the service relays.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServiceToAgent {
    Inject {
        ev: InputEvent,
        display: DisplayInfo,
    },
    Capture {
        display: DisplayInfo,
        out_w: u32,
        out_h: u32,
    },
    Status,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentToService {
    Ready { pid: u32, role: ConnRole },
    Picture(Picture),
    NoPicture,
    Status { secure_desktop: bool },
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("message too large")]
    TooLarge,
    #[error("malformed message")]
    Malformed,
}

pub fn write_msg<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<(), FrameError> {
    let body = postcard::to_stdvec(msg).map_err(|_| FrameError::Malformed)?;
    if body.len() > MAX_MESSAGE {
        return Err(FrameError::TooLarge);
    }
    // One write per message so a pipe never carries a length without its body.
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(&body);
    w.write_all(&buf)?;
    w.flush()?;
    Ok(())
}

pub fn read_msg<R: Read, T: DeserializeOwned>(r: &mut R) -> Result<T, FrameError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_le_bytes(len) as usize;
    if n == 0 || n > MAX_MESSAGE {
        return Err(FrameError::TooLarge);
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)?;
    let (v, rest) = postcard::take_from_bytes::<T>(&body).map_err(|_| FrameError::Malformed)?;
    if !rest.is_empty() {
        return Err(FrameError::Malformed);
    }
    Ok(v)
}

/// Structural checks applied to every request before it is acted on, whoever sent it.
pub fn validate_ui(msg: &UiToService) -> Result<(), &'static str> {
    match msg {
        UiToService::Grant { label } if label.len() > 128 => Err("label too long"),
        UiToService::Input {
            ev: InputEvent::Text(t),
            ..
        } if t.chars().count() > 4096 => Err("text too long"),
        UiToService::Input { display, .. } => check_display(display),
        UiToService::Capture {
            display,
            out_w,
            out_h,
        } => {
            check_display(display)?;
            if *out_w < 16 || *out_h < 16 || *out_w > MAX_DIMENSION || *out_h > MAX_DIMENSION {
                return Err("bad output size");
            }
            if !out_w.is_multiple_of(2) || !out_h.is_multiple_of(2) {
                return Err("output size must be even");
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn check_display(d: &DisplayInfo) -> Result<(), &'static str> {
    if d.width == 0 || d.height == 0 || d.width > MAX_DIMENSION || d.height > MAX_DIMENSION {
        return Err("bad display size");
    }
    if d.name.len() > 256 {
        return Err("display name too long");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display() -> DisplayInfo {
        DisplayInfo {
            index: 0,
            name: "d".into(),
            width: 1920,
            height: 1080,
            x: 0,
            y: 0,
            primary: true,
        }
    }

    #[test]
    fn frames_round_trip_and_reject_trailing_bytes() {
        let mut buf = Vec::new();
        write_msg(&mut buf, &UiToService::Heartbeat).unwrap();
        let got: UiToService = read_msg(&mut buf.as_slice()).unwrap();
        assert!(matches!(got, UiToService::Heartbeat));
        let mut body = postcard::to_stdvec(&UiToService::Heartbeat).unwrap();
        body.push(0);
        let mut framed = (body.len() as u32).to_le_bytes().to_vec();
        framed.extend(body);
        assert!(read_msg::<_, UiToService>(&mut framed.as_slice()).is_err());
    }

    #[test]
    fn oversized_and_empty_frames_are_refused() {
        let huge = ((MAX_MESSAGE + 1) as u32).to_le_bytes();
        assert!(matches!(
            read_msg::<_, UiToService>(&mut huge.as_slice()),
            Err(FrameError::TooLarge)
        ));
        assert!(read_msg::<_, UiToService>(&mut 0u32.to_le_bytes().as_slice()).is_err());
    }

    #[test]
    fn garbage_never_panics() {
        for seed in 0u8..=255 {
            let body: Vec<u8> = (0..48u8)
                .map(|i| i.wrapping_mul(seed).wrapping_add(seed))
                .collect();
            let mut framed = (body.len() as u32).to_le_bytes().to_vec();
            framed.extend(&body);
            let _ = read_msg::<_, UiToService>(&mut framed.as_slice());
            let _ = read_msg::<_, AgentToService>(&mut framed.as_slice());
        }
    }

    #[test]
    fn validation_rejects_hostile_requests() {
        let ok = UiToService::Capture {
            display: display(),
            out_w: 1280,
            out_h: 720,
        };
        assert!(validate_ui(&ok).is_ok());
        let odd = UiToService::Capture {
            display: display(),
            out_w: 1281,
            out_h: 720,
        };
        assert!(validate_ui(&odd).is_err());
        let huge = UiToService::Capture {
            display: display(),
            out_w: 100_000,
            out_h: 720,
        };
        assert!(validate_ui(&huge).is_err());
        let mut bad = display();
        bad.width = 0;
        assert!(validate_ui(&UiToService::Input {
            ev: InputEvent::ReleaseAll,
            display: bad
        })
        .is_err());
        let text = UiToService::Input {
            ev: InputEvent::Text("x".repeat(5000)),
            display: display(),
        };
        assert!(validate_ui(&text).is_err());
        assert!(validate_ui(&UiToService::Grant {
            label: "y".repeat(500)
        })
        .is_err());
    }
}
