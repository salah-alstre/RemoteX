//! The service's local pipe endpoint for the RemoteX desktop application.

use crate::broker::Broker;
use remotex_elevate::{
    pipe::{self, PipeSecurity, USER_PIPE_SDDL},
    read_msg, validate_ui, write_msg, DenyReason, ServiceToUi, UiToService, PROTOCOL_VERSION,
};
use std::{
    fs::File,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

/// More simultaneous connections than the app ever needs; anything beyond this is refused so a local
/// process cannot exhaust the service by opening pipes.
const MAX_CONNECTIONS: usize = 8;

pub fn serve(broker: Arc<Broker>, stop: Arc<AtomicBool>, pipe_name: &str) -> std::io::Result<()> {
    let security = PipeSecurity::from_sddl(USER_PIPE_SDDL)?;
    let live = Arc::new(AtomicUsize::new(0));
    let mut first = true;
    while !stop.load(Ordering::SeqCst) {
        let instance = pipe::create_server(pipe_name, &security, first)?;
        first = false;
        pipe::accept(&instance)?;
        if stop.load(Ordering::SeqCst) {
            break;
        }
        if live.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            live.fetch_sub(1, Ordering::SeqCst);
            pipe::disconnect(&instance);
            continue;
        }
        let (b, l) = (broker.clone(), live.clone());
        std::thread::Builder::new()
            .name("ipc-client".into())
            .spawn(move || {
                let _ = client(&b, instance);
                l.fetch_sub(1, Ordering::SeqCst);
            })?;
    }
    Ok(())
}

/// Unblocks a pending `accept` so `serve` can notice the stop flag.
pub fn wake(pipe_name: &str) {
    let _ = pipe::connect(pipe_name, std::time::Duration::from_millis(200));
}

fn client(broker: &Broker, mut conn: File) -> Result<(), remotex_elevate::FrameError> {
    let (Ok(pid), Ok(session)) = (pipe::client_pid(&conn), pipe::client_session(&conn)) else {
        return Ok(());
    };
    // Who is on the other end is decided by Windows (pipe client process), never by what it says.
    if !broker.caller_allowed(pid, session) {
        // No DisconnectNamedPipe here: it would discard the refusal before the caller reads it.
        let _ = write_msg(&mut conn, &ServiceToUi::Denied(DenyReason::NotAuthorized));
        return Ok(());
    }
    match read_msg::<_, UiToService>(&mut conn)? {
        UiToService::Hello { version } if version == PROTOCOL_VERSION => {
            write_msg(
                &mut conn,
                &ServiceToUi::Welcome {
                    version: PROTOCOL_VERSION,
                    service_version: env!("CARGO_PKG_VERSION").into(),
                },
            )?;
        }
        _ => return Ok(()),
    }

    let mut token: Option<[u8; 16]> = None;
    let mut is_owner = false;
    let result = (|| -> Result<(), remotex_elevate::FrameError> {
        loop {
            let msg: UiToService = read_msg(&mut conn)?;
            if let Err(why) = validate_ui(&msg) {
                tracing::warn!(target: "security", pid, why, "rejected malformed request");
                return Ok(());
            }
            let reply = match msg {
                UiToService::Hello { .. } => return Ok(()),
                UiToService::Grant { label } => {
                    if token.is_some() {
                        Some(ServiceToUi::Denied(DenyReason::AlreadyGranted))
                    } else {
                        match broker.grant(pid, &label) {
                            Ok(t) => {
                                token = Some(t);
                                is_owner = true;
                                Some(ServiceToUi::Granted { token: t })
                            }
                            Err(r) => Some(ServiceToUi::Denied(r)),
                        }
                    }
                }
                UiToService::Attach {
                    token: t,
                    role: _role,
                } => match broker.attach(&t, pid) {
                    Ok(()) => {
                        token = Some(t);
                        Some(ServiceToUi::Attached)
                    }
                    Err(r) => Some(ServiceToUi::Denied(r)),
                },
                UiToService::Revoke => {
                    if let Some(t) = token {
                        broker.revoke(&t, pid);
                    }
                    token = None;
                    Some(ServiceToUi::Revoked)
                }
                UiToService::Heartbeat => match token {
                    Some(t) => broker.heartbeat(&t, pid).err().map(ServiceToUi::Denied),
                    None => Some(ServiceToUi::Denied(DenyReason::NotGranted)),
                },
                // Input has no reply: it must never make the caller wait.
                UiToService::Input { ev, display } => match token {
                    Some(t) => broker.inject(&t, pid, ev, display).err().map(ServiceToUi::Denied),
                    None => Some(ServiceToUi::Denied(DenyReason::NotGranted)),
                },
                UiToService::Capture {
                    display,
                    out_w,
                    out_h,
                } => match token {
                    Some(t) => Some(match broker.capture(&t, pid, display, out_w, out_h) {
                        Ok(Some(p)) => ServiceToUi::Picture(p),
                        Ok(None) => ServiceToUi::NoPicture,
                        Err(r) => ServiceToUi::Denied(r),
                    }),
                    None => Some(ServiceToUi::Denied(DenyReason::NotGranted)),
                },
                UiToService::Status => Some(match token {
                    Some(t) => match broker.status(&t, pid) {
                        Ok(secure_desktop) => ServiceToUi::Status {
                            secure_desktop,
                            agent_running: true,
                        },
                        Err(r) => ServiceToUi::Denied(r),
                    },
                    None => ServiceToUi::Status {
                        secure_desktop: false,
                        agent_running: false,
                    },
                }),
            };
            if let Some(r) = reply {
                write_msg(&mut conn, &r)?;
            }
        }
    })();
    // The owner going away (crash, exit, disconnect) revokes at once.
    if is_owner {
        broker.owner_gone(pid);
    }
    result
}
