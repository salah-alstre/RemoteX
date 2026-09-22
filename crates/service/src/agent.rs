//! The agent: a short-lived helper the service starts inside the user's session with a SYSTEM token.
//!
//! It exists only while an elevated session is active. It is a pure executor: it connects *out* to two
//! private pipes created by the service (nobody else can reach it), runs exactly the operations in
//! [`ServiceToAgent`], and exits as soon as either pipe closes.

use crate::desktop::{self, DesktopAttachment};
use remotex_elevate::{pipe, read_msg, write_msg, AgentToService, ConnRole, ServiceToAgent};
use remotex_input::injector::Injector;
use std::{io, time::Duration};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};

pub fn run(input_pipe: &str, video_pipe: &str) -> io::Result<()> {
    // Coordinates must be physical pixels, like the pictures and the injected positions.
    // SAFETY: process-wide DPI setting, done once before any window/GDI work.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let (i, v) = (input_pipe.to_string(), video_pipe.to_string());
    let input = std::thread::Builder::new()
        .name("agent-input".into())
        .spawn(move || input_loop(&i))?;
    let video = std::thread::Builder::new()
        .name("agent-video".into())
        .spawn(move || video_loop(&v))?;
    // Either loop ending (pipe closed, service gone) ends the agent: it never lingers with privileges.
    loop {
        if input.is_finished() || video.is_finished() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn input_loop(name: &str) -> io::Result<()> {
    let mut conn = pipe::connect(name, Duration::from_secs(10))?;
    write_msg(
        &mut conn,
        &AgentToService::Ready {
            pid: std::process::id(),
            role: ConnRole::Input,
        },
    )
    .map_err(io::Error::other)?;
    let mut desk = DesktopAttachment::default();
    let mut injector = Injector::new();
    loop {
        match read_msg::<_, ServiceToAgent>(&mut conn) {
            Ok(ServiceToAgent::Inject { ev, display }) => {
                // The input desktop can change at any moment (a UAC prompt appears): follow it first.
                let _ = desk.follow_input_desktop();
                injector.apply(&ev, &display);
            }
            Ok(ServiceToAgent::Status) => {
                let _ = desk.follow_input_desktop();
                write_msg(
                    &mut conn,
                    &AgentToService::Status {
                        secure_desktop: desk.is_secure(),
                    },
                )
                .map_err(io::Error::other)?;
            }
            Ok(ServiceToAgent::Shutdown) | Err(_) => {
                injector.release_all();
                return Ok(());
            }
            Ok(ServiceToAgent::Capture { .. }) => {}
        }
    }
}

fn video_loop(name: &str) -> io::Result<()> {
    let mut conn = pipe::connect(name, Duration::from_secs(10))?;
    write_msg(
        &mut conn,
        &AgentToService::Ready {
            pid: std::process::id(),
            role: ConnRole::Video,
        },
    )
    .map_err(io::Error::other)?;
    let mut desk = DesktopAttachment::default();
    loop {
        match read_msg::<_, ServiceToAgent>(&mut conn) {
            Ok(ServiceToAgent::Capture {
                display,
                out_w,
                out_h,
            }) => {
                let _ = desk.follow_input_desktop();
                let reply = match desktop::capture(&display, out_w, out_h) {
                    Ok(p) => AgentToService::Picture(p),
                    Err(_) => AgentToService::NoPicture,
                };
                write_msg(&mut conn, &reply).map_err(io::Error::other)?;
            }
            Ok(ServiceToAgent::Status) => {
                let _ = desk.follow_input_desktop();
                write_msg(
                    &mut conn,
                    &AgentToService::Status {
                        secure_desktop: desk.is_secure(),
                    },
                )
                .map_err(io::Error::other)?;
            }
            Ok(ServiceToAgent::Shutdown) | Err(_) => return Ok(()),
            Ok(ServiceToAgent::Inject { .. }) => {}
        }
    }
}
