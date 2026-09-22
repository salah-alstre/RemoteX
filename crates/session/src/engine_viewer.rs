//! Viewer-side connection procedure with automatic reconnection.

use crate::{
    engine::{ConnectResult, Inner},
    handshake::{viewer_handshake, ViewerFailure, ViewerParams},
    link::{self, Link},
    secure,
    types::{EndReason, Event, FailReason, LinkInfo, ViewerReady, ViewerState},
    viewer::{self, ViewerCmd, ViewerHandle, ViewerSession},
};
use remotex_common::{
    peer::{AbortReason, AuthKind, Msg, Permissions, RejectReason},
    signaling::{ClientToServer, ConnectFailure},
    DeviceId, SessionId,
};
use remotex_security::password::normalize_temp_password;
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    time::timeout,
};

/// Delay before reconnect attempt `n` (1-based): exponential with a ceiling.
pub fn backoff_secs(attempt: u32) -> u32 {
    (1u32 << attempt.saturating_sub(1).min(5)).min(30)
}

pub const MAX_RECONNECT_ATTEMPTS: u32 = 8;

fn map_connect(f: ConnectFailure) -> FailReason {
    match f {
        ConnectFailure::Offline => FailReason::Offline,
        ConnectFailure::NotAccepting => FailReason::NotAccepting,
        ConnectFailure::RateLimited => FailReason::RateLimited,
        ConnectFailure::Banned => FailReason::Banned,
        ConnectFailure::Busy => FailReason::Busy,
        ConnectFailure::InvalidId => FailReason::InvalidId,
    }
}

fn map_abort(a: AbortReason) -> FailReason {
    match a {
        AbortReason::Busy => FailReason::Busy,
        AbortReason::Unsupported => FailReason::Unsupported,
        AbortReason::NotAccepting => FailReason::NotAccepting,
        AbortReason::LockedOut => FailReason::LockedOut,
        AbortReason::AuthFailed => FailReason::WrongPassword,
        AbortReason::UnattendedDisabled => FailReason::UnattendedDisabled,
    }
}

/// Whether retrying could possibly help.
fn retryable(r: FailReason) -> bool {
    matches!(
        r,
        FailReason::ServerUnavailable
            | FailReason::ConnectionLost
            | FailReason::Protocol
            | FailReason::Timeout
    )
}

struct Target {
    id: DeviceId,
    password: String,
    auth: AuthKind,
    requested: Permissions,
}

pub(crate) fn start(
    inner: &Arc<Inner>,
    target: &str,
    password: &str,
    auth: AuthKind,
    requested: Permissions,
) -> Result<u64, ConnectFailure> {
    let id = DeviceId::parse(target).ok_or(ConnectFailure::InvalidId)?;
    let session_no = inner.next_session_no.fetch_add(1, Ordering::Relaxed);
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let input_slot: Arc<std::sync::Mutex<Option<secure::SecureTx>>> = Arc::default();
    inner.viewers.lock().unwrap_or_else(|p| p.into_inner()).insert(
        session_no,
        ViewerHandle {
            cmd: cmd_tx,
            input: input_slot.clone(),
        },
    );
    let pw = match auth {
        AuthKind::Temporary => normalize_temp_password(password),
        AuthKind::Permanent => password.trim().to_string(),
    };
    let t = Target {
        id,
        password: pw,
        auth,
        requested,
    };
    let inner = inner.clone();
    let rt = inner.rt.clone();
    rt.spawn(async move {
        supervise(inner.clone(), session_no, t, cmd_rx, input_slot).await;
        inner
            .viewers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&session_no);
    });
    Ok(session_no)
}

enum Attempt {
    Ended(EndReason),
    Failed(FailReason),
}

async fn supervise(
    inner: Arc<Inner>,
    no: u64,
    t: Target,
    mut cmd_rx: mpsc::UnboundedReceiver<ViewerCmd>,
    input_slot: Arc<std::sync::Mutex<Option<secure::SecureTx>>>,
) {
    let mut ever_connected = false;
    let mut attempt_no = 0u32;
    loop {
        let (outcome, connected) = attempt(&inner, no, &t, &mut cmd_rx, &input_slot).await;
        *input_slot.lock().unwrap_or_else(|p| p.into_inner()) = None;
        ever_connected |= connected;
        match outcome {
            Attempt::Ended(EndReason::LocalClosed) => break,
            Attempt::Ended(EndReason::PeerClosed | EndReason::Kicked) => {
                inner.emit(Event::Viewer {
                    session: no,
                    state: ViewerState::Closed,
                });
                break;
            }
            Attempt::Ended(EndReason::ConnectionLost) => {}
            Attempt::Failed(r) if !(ever_connected && retryable(r)) => {
                inner.emit(Event::Viewer {
                    session: no,
                    state: ViewerState::Failed { reason: r },
                });
                break;
            }
            Attempt::Failed(_) => {}
        }
        if connected {
            attempt_no = 0;
        }
        attempt_no += 1;
        if attempt_no > MAX_RECONNECT_ATTEMPTS {
            inner.emit(Event::Viewer {
                session: no,
                state: ViewerState::Failed {
                    reason: FailReason::ConnectionLost,
                },
            });
            // Stay alive so the UI's Reconnect button can start over; a Disconnect command ends the task.
            match wait_for_user(&mut cmd_rx).await {
                UserChoice::Reconnect => {
                    attempt_no = 0;
                    continue;
                }
                UserChoice::Close => break,
            }
        }
        let delay = backoff_secs(attempt_no);
        inner.emit(Event::Viewer {
            session: no,
            state: ViewerState::Reconnecting {
                attempt: attempt_no,
                next_in_secs: delay,
            },
        });
        // Sleep, but honour a Disconnect issued while waiting.
        let sleep = tokio::time::sleep(Duration::from_secs(delay as u64));
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => break,
                Some(cmd) = cmd_rx.recv() => {
                    if matches!(cmd, ViewerCmd::Disconnect) {
                        inner.emit(Event::Viewer { session: no, state: ViewerState::Closed });
                        return;
                    }
                }
            }
        }
    }
}

enum UserChoice {
    Reconnect,
    Close,
}

/// After exhausting automatic retries: wait for Reconnect (a keyframe request is the UI's signal) or Disconnect.
async fn wait_for_user(rx: &mut mpsc::UnboundedReceiver<ViewerCmd>) -> UserChoice {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            ViewerCmd::Disconnect => return UserChoice::Close,
            ViewerCmd::RequestKeyframe => return UserChoice::Reconnect,
            _ => {}
        }
    }
    UserChoice::Close
}

async fn attempt(
    inner: &Arc<Inner>,
    no: u64,
    t: &Target,
    cmd_rx: &mut mpsc::UnboundedReceiver<ViewerCmd>,
    input_slot: &Arc<std::sync::Mutex<Option<secure::SecureTx>>>,
) -> (Attempt, bool) {
    macro_rules! fail {
        ($r:expr) => {
            return (Attempt::Failed($r), false)
        };
    }
    let cfg = inner.cfg();
    let my_id = inner.my_id.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let Some(my_id) = my_id.filter(|_| *inner.server_online.lock().unwrap_or_else(|p| p.into_inner())) else {
        fail!(FailReason::ServerUnavailable)
    };

    inner.emit(Event::Viewer {
        session: no,
        state: ViewerState::Connecting,
    });
    let (ready_tx, ready_rx) = oneshot::channel::<ConnectResult>();
    inner
        .pending_connect
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(t.id.as_str().to_string(), ready_tx);
    if !inner
        .sig
        .send(ClientToServer::Connect {
            target_id: t.id.as_str().to_string(),
        })
        .await
    {
        fail!(FailReason::ServerUnavailable)
    }
    let ready = match timeout(Duration::from_secs(12), ready_rx).await {
        Ok(Ok(Ok(r))) => r,
        Ok(Ok(Err(f))) => fail!(map_connect(f)),
        _ => {
            inner
                .pending_connect
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clear();
            fail!(FailReason::ServerUnavailable)
        }
    };
    let session: SessionId = ready.session;

    // Direct first (same network), relay otherwise.
    let direct = if cfg.direct_enabled && !ready.candidates.is_empty() {
        link::connect_direct(&ready.candidates, session.0, Duration::from_millis(1500)).await
    } else {
        None
    };
    let link: Link = match direct {
        Some(l) => l,
        None if cfg.relay_fallback => inner.relay_link(session),
        None => {
            cleanup(inner, session).await;
            fail!(FailReason::Offline)
        }
    };
    inner
        .sig
        .send(ClientToServer::SessionMode {
            session,
            direct: link.kind == link::LinkKind::Direct,
        })
        .await;
    let link_info = LinkInfo {
        kind: link.kind,
        peer: link.peer.clone(),
        protocol: if link.kind == link::LinkKind::Direct {
            "tcp"
        } else {
            "tls-websocket"
        },
    };

    inner.emit(Event::Viewer {
        session: no,
        state: ViewerState::Authenticating,
    });
    let mut link = link;
    let params = ViewerParams {
        session,
        viewer_id: &my_id,
        viewer_name: &cfg.device_name,
        host_id: t.id.as_str(),
        password: &t.password,
        auth: t.auth,
        requested: t.requested,
    };
    let channel = match viewer_handshake(&mut link, &params).await {
        Ok(c) => c,
        Err(e) => {
            cleanup(inner, session).await;
            fail!(match e {
                ViewerFailure::WrongPassword => FailReason::WrongPassword,
                ViewerFailure::Aborted(a) => map_abort(a),
                ViewerFailure::Protocol => FailReason::Protocol,
            })
        }
    };

    let (tx, mut rx, closed) = secure::spawn(link, channel, None);
    inner.emit(Event::Viewer {
        session: no,
        state: ViewerState::WaitingForApproval,
    });
    let wait = cfg.approval_timeout + Duration::from_secs(15);
    let first = timeout(wait, rx.recv()).await;
    let (permissions, displays, active_display, host_name) = match first {
        Ok(Some(Msg::Accepted {
            permissions,
            displays,
            active_display,
            host_name,
        })) => (permissions, displays, active_display, host_name),
        Ok(Some(Msg::Rejected(r))) => {
            cleanup(inner, session).await;
            fail!(match r {
                RejectReason::Declined => FailReason::Declined,
                RejectReason::Timeout => FailReason::Timeout,
                RejectReason::Busy => FailReason::Busy,
            })
        }
        Err(_) => {
            cleanup(inner, session).await;
            fail!(FailReason::Timeout)
        }
        _ => {
            cleanup(inner, session).await;
            fail!(FailReason::Protocol)
        }
    };

    inner.emit(Event::ViewerReady {
        session: no,
        info: ViewerReady {
            host_name,
            displays: displays.clone(),
            permissions,
            active_display,
            link: link_info,
        },
    });
    inner.emit(Event::Viewer {
        session: no,
        state: ViewerState::Connected,
    });

    tracing::info!(target: "session", role = "viewer", session = no, "remote session started");
    // From now on input events skip the session loop and go straight to the send queue.
    *input_slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx.clone());
    let reason = viewer::run(ViewerSession {
        session_no: no,
        env: inner.env.clone(),
        events: inner.events.clone(),
        cfg: inner.cfg(),
        tx,
        rx,
        closed,
        cmd_rx,
        permissions,
        displays,
    })
    .await;
    tracing::info!(target: "session", role = "viewer", session = no, ?reason, "remote session ended");
    cleanup(inner, session).await;
    (Attempt::Ended(reason), true)
}

async fn cleanup(inner: &Inner, session: SessionId) {
    inner
        .sig
        .send(ClientToServer::EndSession {
            session,
            auth_failed: false,
        })
        .await;
    inner.drop_route(&session);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_exponential_and_capped() {
        let seq: Vec<u32> = (1..=8).map(backoff_secs).collect();
        assert_eq!(seq, vec![1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn only_transient_failures_are_retried() {
        assert!(retryable(FailReason::ConnectionLost));
        assert!(!retryable(FailReason::WrongPassword));
        assert!(!retryable(FailReason::Declined));
        assert!(!retryable(FailReason::Banned));
    }
}
