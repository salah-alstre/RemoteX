//! Client side of elevated control: talks to the RemoteX Windows service over its local pipe.
//!
//! Two pipe connections are used so a slow picture request never delays input: one carries input and the
//! grant itself (plus a heartbeat), the other carries picture requests. Dropping the guard sends a
//! revocation; if the app dies instead, the service notices the closed pipe and revokes on its own.

use crate::env::{
    ElevatedInput, ElevatedPicture, ElevatedSession, ElevatedVideo, ElevationProvider, Frame, Pointer,
};
use remotex_common::peer::{DisplayInfo, InputEvent};
use remotex_elevate::{
    pipe, read_msg, write_msg, ConnRole, ServiceToUi, UiToService, PROTOCOL_VERSION, SERVICE_PIPE,
};
use std::{
    fs::File,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

pub struct ServiceElevation {
    pipe: String,
}

impl ServiceElevation {
    pub fn new() -> Self {
        Self {
            pipe: SERVICE_PIPE.to_string(),
        }
    }

    /// Talk to a different pipe (development builds and tests only).
    pub fn with_pipe(name: &str) -> Self {
        Self {
            pipe: name.to_string(),
        }
    }

    fn open(&self) -> Result<File, String> {
        let mut c = pipe::connect(&self.pipe, Duration::from_millis(500)).map_err(|e| e.to_string())?;
        write_msg(
            &mut c,
            &UiToService::Hello {
                version: PROTOCOL_VERSION,
            },
        )
        .map_err(|e| e.to_string())?;
        match read_msg::<_, ServiceToUi>(&mut c).map_err(|e| e.to_string())? {
            ServiceToUi::Welcome { .. } => Ok(c),
            other => Err(format!("service refused the connection: {other:?}")),
        }
    }
}

impl Default for ServiceElevation {
    fn default() -> Self {
        Self::new()
    }
}

impl ElevationProvider for ServiceElevation {
    fn available(&self) -> bool {
        self.open().is_ok()
    }

    fn begin(&self, label: &str) -> Result<ElevatedSession, String> {
        let mut input = self.open()?;
        write_msg(
            &mut input,
            &UiToService::Grant {
                label: label.chars().take(64).collect(),
            },
        )
        .map_err(|e| e.to_string())?;
        let token = match read_msg::<_, ServiceToUi>(&mut input).map_err(|e| e.to_string())? {
            ServiceToUi::Granted { token } => token,
            other => return Err(format!("elevated control was not granted: {other:?}")),
        };
        let mut video = self.open()?;
        write_msg(
            &mut video,
            &UiToService::Attach {
                token,
                role: ConnRole::Video,
            },
        )
        .map_err(|e| e.to_string())?;
        match read_msg::<_, ServiceToUi>(&mut video).map_err(|e| e.to_string())? {
            ServiceToUi::Attached => {}
            other => return Err(format!("could not attach the picture channel: {other:?}")),
        }

        let alive = Arc::new(AtomicBool::new(true));
        let writer = Arc::new(Mutex::new(input.try_clone().map_err(|e| e.to_string())?));
        // Unsolicited answers on the input pipe mean the service ended the grant.
        let (a2, mut reader) = (alive.clone(), input);
        std::thread::Builder::new()
            .name("elevation-events".into())
            .spawn(move || {
                while let Ok(msg) = read_msg::<_, ServiceToUi>(&mut reader) {
                    if matches!(msg, ServiceToUi::Denied(_) | ServiceToUi::Revoked) {
                        break;
                    }
                }
                a2.store(false, Ordering::SeqCst);
            })
            .map_err(|e| e.to_string())?;
        // Heartbeat: the service revokes on its own if these stop.
        let (a3, w3) = (alive.clone(), writer.clone());
        std::thread::Builder::new()
            .name("elevation-heartbeat".into())
            .spawn(move || {
                while a3.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_secs(3));
                    let ok = w3
                        .lock()
                        .map(|mut f| write_msg(&mut *f, &UiToService::Heartbeat).is_ok())
                        .unwrap_or(false);
                    if !ok {
                        a3.store(false, Ordering::SeqCst);
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        Ok(ElevatedSession {
            input: Box::new(PipeInput {
                writer: writer.clone(),
                alive: alive.clone(),
            }),
            video: Box::new(PipeVideo {
                conn: video,
                alive: alive.clone(),
            }),
            guard: Box::new(Guard { writer, alive }),
        })
    }
}

struct PipeInput {
    writer: Arc<Mutex<File>>,
    alive: Arc<AtomicBool>,
}

impl ElevatedInput for PipeInput {
    fn apply(&mut self, ev: &InputEvent, display: &DisplayInfo) {
        let ok = self
            .writer
            .lock()
            .map(|mut f| {
                write_msg(
                    &mut *f,
                    &UiToService::Input {
                        ev: ev.clone(),
                        display: display.clone(),
                    },
                )
                .is_ok()
            })
            .unwrap_or(false);
        if !ok {
            self.alive.store(false, Ordering::SeqCst);
        }
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

struct PipeVideo {
    conn: File,
    alive: Arc<AtomicBool>,
}

impl ElevatedVideo for PipeVideo {
    fn secure_desktop(&mut self) -> bool {
        if write_msg(&mut self.conn, &UiToService::Status).is_err() {
            self.alive.store(false, Ordering::SeqCst);
            return false;
        }
        matches!(
            read_msg::<_, ServiceToUi>(&mut self.conn),
            Ok(ServiceToUi::Status {
                secure_desktop: true,
                ..
            })
        )
    }

    fn capture(&mut self, display: &DisplayInfo, out_w: u32, out_h: u32) -> Option<ElevatedPicture> {
        write_msg(
            &mut self.conn,
            &UiToService::Capture {
                display: display.clone(),
                out_w,
                out_h,
            },
        )
        .ok()?;
        match read_msg::<_, ServiceToUi>(&mut self.conn).ok()? {
            ServiceToUi::Picture(p) => Some(ElevatedPicture {
                frame: Frame {
                    width: p.width,
                    height: p.height,
                    data: p.nv12,
                },
                pointer: p.pointer.map(|(x, y, visible)| Pointer {
                    x: x - display.x,
                    y: y - display.y,
                    visible,
                }),
            }),
            _ => None,
        }
    }
}

/// Dropping this ends the grant at the service, and with it the privileged helper.
struct Guard {
    writer: Arc<Mutex<File>>,
    alive: Arc<AtomicBool>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        if let Ok(mut f) = self.writer.lock() {
            let _ = write_msg(&mut *f, &UiToService::Revoke);
        }
    }
}
