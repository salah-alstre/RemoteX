//! Host-side handling of an incoming connection request announced by the server.

use crate::{
    engine::Inner,
    handshake::{host_handshake, HostFailure, HostParams, PasswordLookup},
    host::{self, HostCmd, HostHandle, HostSession},
    link::{self, Link},
    types::{EndReason, Event, HostNotice, IncomingRequest},
};
use remotex_common::{
    peer::{AuthKind, Permissions, RejectReason},
    signaling::ClientToServer,
    SessionId,
};
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc, oneshot},
    time::timeout,
};

/// How long a dropped viewer may resume without a new approval prompt.
const RESUME_WINDOW: Duration = Duration::from_secs(300);

async fn end_on_server(inner: &Inner, session: SessionId, auth_failed: bool) {
    inner
        .sig
        .send(ClientToServer::EndSession { session, auth_failed })
        .await;
    inner.drop_route(&session);
    inner
        .pending_direct
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&session);
}

/// Waits for whichever transport the viewer chose: a direct TCP connection or the first relayed frame.
async fn await_link(inner: &Inner, session: SessionId) -> Option<Link> {
    let mut relay = inner.relay_link(session);
    let (direct_tx, direct_rx) = oneshot::channel();
    inner
        .pending_direct
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(session, direct_tx);
    tokio::select! {
        stream = direct_rx => stream.ok().map(|s| link::from_tcp(s, link::LinkKind::Direct)),
        first = timeout(Duration::from_secs(20), relay.rx.recv()) => {
            let first = first.ok()??;
            relay.pending = Some(first);
            Some(relay)
        }
    }
}

pub(crate) async fn handle_incoming(
    inner: Arc<Inner>,
    session: SessionId,
    from_id: String,
    from_name: String,
    _same_network: bool,
) {
    let cfg = inner.cfg();
    let my_id = inner
        .my_id
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .unwrap_or_default();

    let accept = {
        let mut h = inner.host();
        if !*inner.incoming_enabled.borrow() || h.busy {
            false
        } else {
            h.busy = true;
            true
        }
    };
    if !accept {
        end_on_server(&inner, session, false).await;
        return;
    }
    let mut released = ReleaseBusy(Some(inner.clone()));

    let Some(mut link) = await_link(&inner, session).await else {
        end_on_server(&inner, session, false).await;
        return;
    };

    let params = HostParams {
        session,
        host_id: &my_id,
        expected_viewer_id: &from_id,
    };
    let inner_lookup = inner.clone();
    let authed = host_handshake(&mut link, &params, move |viewer_id, kind| {
        let mut h = inner_lookup.host();
        if h.per_viewer_failures
            .locked(&viewer_id.to_string(), Instant::now())
            .is_some()
        {
            return PasswordLookup::LockedOut;
        }
        match kind {
            AuthKind::Temporary => PasswordLookup::Available(h.temp.clone()),
            AuthKind::Permanent => match (&h.permanent_password, h.unattended_enabled) {
                (Some(pw), true) if h.unattended_any_device || h.authorized.contains(viewer_id) => {
                    PasswordLookup::Available(pw.clone())
                }
                _ => PasswordLookup::Disabled,
            },
        }
    })
    .await;

    let auth = match authed {
        Ok(a) => a,
        Err(HostFailure::BadPassword { viewer_id }) => {
            let (locked, rotate) = {
                let mut h = inner.host();
                let now = Instant::now();
                let locked = h.per_viewer_failures.record_failure(&viewer_id, now);
                // Many failures across different viewers suggest a distributed guess: burn the password.
                let rotate = h.global_failures.record_failure(&(), now);
                (locked, rotate)
            };
            tracing::warn!(target: "security", viewer = %viewer_id, locked, "incorrect password presented");
            inner.emit(Event::HostNotice {
                notice: HostNotice::PasswordRejected { viewer_id },
            });
            if locked {
                inner.emit(Event::HostNotice {
                    notice: HostNotice::LockedOut,
                });
            }
            if rotate {
                inner.rotate_after_session();
            }
            // Free the host for the next attempt before the slower teardown steps.
            released.release();
            link.drain().await;
            end_on_server(&inner, session, true).await;
            return;
        }
        Err(_) => {
            released.release();
            link.drain().await;
            end_on_server(&inner, session, false).await;
            return;
        }
    };
    inner.host().per_viewer_failures.record_success(&auth.viewer_id);

    // Consent: unattended access with the permanent password, or a recent dropped session, needs no prompt.
    let auto = {
        let h = inner.host();
        let unattended = auth.auth == AuthKind::Permanent && h.unattended_enabled;
        // (Authorization was already enforced when the permanent password was offered.)
        let resume = h
            .resume_grants
            .get(&auth.viewer_id)
            .filter(|(_, at)| at.elapsed() < RESUME_WINDOW)
            .map(|(p, _)| *p);
        // Elevated control is never implied: unattended sessions get it only if the owner enabled that
        // separately, and a dropped session that resumes without a prompt never gets it back.
        if unattended {
            Some(Permissions {
                elevated: auth.requested.elevated && h.unattended_elevated,
                ..auth.requested
            })
        } else {
            resume.map(|p| Permissions {
                elevated: false,
                ..p.intersect(auth.requested)
            })
        }
    };
    let granted = match auto {
        Some(p) => Some(p),
        None => {
            let request_id = inner.next_request.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = oneshot::channel();
            inner.host().pending.insert(request_id, tx);
            inner.emit(Event::Incoming(IncomingRequest {
                request_id,
                viewer_id: auth.viewer_id.clone(),
                viewer_name: from_name.clone(),
                requested: auth.requested,
                same_network: link.kind == link::LinkKind::Direct,
                timeout_secs: cfg.approval_timeout.as_secs(),
            }));
            let decision = timeout(cfg.approval_timeout, rx).await;
            inner.host().pending.remove(&request_id);
            inner.emit(Event::IncomingCancelled { request_id });
            match decision {
                Ok(Ok(Some(p))) => Some(p.intersect(auth.requested)),
                Ok(_) => {
                    let (a, l) = (auth, link);
                    inner.emit(Event::HostNotice {
                        notice: HostNotice::Declined {
                            viewer_id: a.viewer_id.clone(),
                        },
                    });
                    host::reject(a, l, RejectReason::Declined).await;
                    end_on_server(&inner, session, false).await;
                    return;
                }
                Err(_) => {
                    host::reject(auth, link, RejectReason::Timeout).await;
                    end_on_server(&inner, session, false).await;
                    return;
                }
            }
        }
    };
    let granted = granted.unwrap_or(Permissions::NONE);
    // Elevated control alone (without any standard permission) is not a session.
    if !granted.view && !granted.mouse && !granted.keyboard && !granted.clipboard && !granted.files {
        host::reject(auth, link, RejectReason::Declined).await;
        end_on_server(&inner, session, false).await;
        return;
    }

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<HostCmd>();
    inner.host().active = Some(HostHandle { cmd: cmd_tx });
    let viewer_id = auth.viewer_id.clone();
    let session_no = inner.next_session_no.fetch_add(1, Ordering::Relaxed);
    let reason = host::run(HostSession {
        session_no,
        env: inner.env.clone(),
        events: inner.events.clone(),
        cfg: cfg.clone(),
        host_id: my_id,
        host_name: cfg.device_name.clone(),
        auth,
        link,
        granted,
        cmd_rx,
    })
    .await;

    {
        let mut h = inner.host();
        h.active = None;
        if reason == EndReason::ConnectionLost {
            h.resume_grants.insert(viewer_id, (granted, Instant::now()));
        } else {
            h.resume_grants.remove(&viewer_id);
        }
    }
    inner.emit(Event::HostEnded { reason });
    end_on_server(&inner, session, false).await;
    if reason != EndReason::ConnectionLost && cfg.rotate_after_session {
        inner.rotate_after_session();
    }
    drop(released);
}

/// Clears the busy flag on every exit path.
struct ReleaseBusy(Option<Arc<Inner>>);

impl ReleaseBusy {
    fn release(&mut self) {
        if let Some(inner) = self.0.take() {
            inner.host().busy = false;
        }
    }
}

impl Drop for ReleaseBusy {
    fn drop(&mut self) {
        self.release();
    }
}
