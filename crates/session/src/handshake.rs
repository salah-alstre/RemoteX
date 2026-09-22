//! Session establishment: PAKE authentication with key confirmation, host-side policy checks.

use crate::link::Link;
use remotex_common::{
    peer::{AbortReason, AuthKind, Handshake, Permissions},
    signaling::{MAX_NAME_LEN, PROTOCOL_VERSION},
    wire, SessionId,
};
use remotex_security::{PakeState, Role, SecureChannel};
use std::time::Duration;
use tokio::time::timeout;

const STEP_TIMEOUT: Duration = Duration::from_secs(15);

async fn send(link: &Link, h: &Handshake) -> bool {
    match wire::encode(h) {
        Ok(b) => link.tx.send(b).await.is_ok(),
        Err(_) => false,
    }
}

async fn recv(link: &mut Link) -> Option<Handshake> {
    let frame = timeout(STEP_TIMEOUT, link.recv()).await.ok()??;
    wire::decode(&frame).ok()
}

#[derive(Debug, PartialEq, Eq)]
pub enum ViewerFailure {
    Aborted(AbortReason),
    WrongPassword,
    Protocol,
}

pub struct ViewerParams<'a> {
    pub session: SessionId,
    pub viewer_id: &'a str,
    pub viewer_name: &'a str,
    pub host_id: &'a str,
    pub password: &'a str,
    pub auth: AuthKind,
    pub requested: Permissions,
}

pub async fn viewer_handshake(link: &mut Link, p: &ViewerParams<'_>) -> Result<SecureChannel, ViewerFailure> {
    let pake = PakeState::start(
        Role::Viewer,
        p.password.as_bytes(),
        &p.session,
        p.viewer_id,
        p.host_id,
    );
    let hello = Handshake::Hello {
        version: PROTOCOL_VERSION,
        viewer_id: p.viewer_id.to_string(),
        viewer_name: p.viewer_name.to_string(),
        auth: p.auth,
        pake: pake.message().to_vec(),
        requested: p.requested,
    };
    if !send(link, &hello).await {
        return Err(ViewerFailure::Protocol);
    }
    let peer_msg = match recv(link).await {
        Some(Handshake::HelloReply { pake }) => pake,
        Some(Handshake::Abort { reason }) => return Err(ViewerFailure::Aborted(reason)),
        _ => return Err(ViewerFailure::Protocol),
    };
    let keys = pake.finish(&peer_msg).map_err(|_| ViewerFailure::Protocol)?;
    if !send(
        link,
        &Handshake::Confirm {
            mac: keys.my_confirm(),
        },
    )
    .await
    {
        return Err(ViewerFailure::Protocol);
    }
    match recv(link).await {
        Some(Handshake::Confirm { mac }) if keys.verify_peer_confirm(&mac) => Ok(keys.into_channel()),
        Some(Handshake::Confirm { .. })
        | Some(Handshake::Abort {
            reason: AbortReason::AuthFailed,
        }) => Err(ViewerFailure::WrongPassword),
        Some(Handshake::Abort { reason }) => Err(ViewerFailure::Aborted(reason)),
        _ => Err(ViewerFailure::Protocol),
    }
}

/// What the host needs to decide about an authentication attempt, supplied by the engine.
pub enum PasswordLookup {
    Available(String),
    /// Unattended access requested but not enabled.
    Disabled,
    LockedOut,
}

pub struct HostParams<'a> {
    pub session: SessionId,
    pub host_id: &'a str,
    /// Identity the server attested for the viewer; the Hello must match it.
    pub expected_viewer_id: &'a str,
}

pub struct Authenticated {
    pub channel: SecureChannel,
    pub viewer_id: String,
    pub viewer_name: String,
    pub auth: AuthKind,
    pub requested: Permissions,
}

pub enum HostFailure {
    /// Wrong password: counts towards lockout.
    BadPassword {
        viewer_id: String,
    },
    Refused(AbortReason),
    Protocol,
}

pub async fn host_handshake(
    link: &mut Link,
    p: &HostParams<'_>,
    lookup: impl FnOnce(&str, AuthKind) -> PasswordLookup,
) -> Result<Authenticated, HostFailure> {
    let Some(Handshake::Hello {
        version,
        viewer_id,
        viewer_name,
        auth,
        pake,
        requested,
    }) = recv(link).await
    else {
        return Err(HostFailure::Protocol);
    };
    if version != PROTOCOL_VERSION {
        let _ = send(
            link,
            &Handshake::Abort {
                reason: AbortReason::Unsupported,
            },
        )
        .await;
        return Err(HostFailure::Refused(AbortReason::Unsupported));
    }
    if viewer_id != p.expected_viewer_id
        || viewer_name.is_empty()
        || viewer_name.len() > MAX_NAME_LEN
        || viewer_name.chars().any(char::is_control)
    {
        return Err(HostFailure::Protocol);
    }
    let password = match lookup(&viewer_id, auth) {
        PasswordLookup::Available(pw) => pw,
        PasswordLookup::Disabled => {
            let _ = send(
                link,
                &Handshake::Abort {
                    reason: AbortReason::UnattendedDisabled,
                },
            )
            .await;
            return Err(HostFailure::Refused(AbortReason::UnattendedDisabled));
        }
        PasswordLookup::LockedOut => {
            let _ = send(
                link,
                &Handshake::Abort {
                    reason: AbortReason::LockedOut,
                },
            )
            .await;
            return Err(HostFailure::Refused(AbortReason::LockedOut));
        }
    };
    let state = PakeState::start(Role::Host, password.as_bytes(), &p.session, &viewer_id, p.host_id);
    if !send(
        link,
        &Handshake::HelloReply {
            pake: state.message().to_vec(),
        },
    )
    .await
    {
        return Err(HostFailure::Protocol);
    }
    let keys = state.finish(&pake).map_err(|_| HostFailure::Protocol)?;
    let Some(Handshake::Confirm { mac }) = recv(link).await else {
        return Err(HostFailure::Protocol);
    };
    if !keys.verify_peer_confirm(&mac) {
        let _ = send(
            link,
            &Handshake::Abort {
                reason: AbortReason::AuthFailed,
            },
        )
        .await;
        return Err(HostFailure::BadPassword { viewer_id });
    }
    if !send(
        link,
        &Handshake::Confirm {
            mac: keys.my_confirm(),
        },
    )
    .await
    {
        return Err(HostFailure::Protocol);
    }
    Ok(Authenticated {
        channel: keys.into_channel(),
        viewer_id,
        viewer_name,
        auth,
        requested,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::{memory_pair, LinkKind};
    use remotex_security::ids::generate_session_id;

    async fn run(
        host_pw: &str,
        viewer_pw: &str,
        spoof_id: bool,
    ) -> (
        Result<Authenticated, HostFailure>,
        Result<SecureChannel, ViewerFailure>,
    ) {
        let (mut vl, mut hl) = memory_pair(LinkKind::Direct);
        let session = generate_session_id();
        let hp = host_pw.to_string();
        let expected = if spoof_id { "999999999" } else { "111111111" };
        let host = tokio::spawn(async move {
            let p = HostParams {
                session,
                host_id: "222222222",
                expected_viewer_id: expected,
            };
            host_handshake(&mut hl, &p, |_, _| PasswordLookup::Available(hp)).await
        });
        let vp = ViewerParams {
            session,
            viewer_id: "111111111",
            viewer_name: "Viewer",
            host_id: "222222222",
            password: viewer_pw,
            auth: AuthKind::Temporary,
            requested: Permissions::ALL,
        };
        let v = viewer_handshake(&mut vl, &vp).await;
        (host.await.unwrap(), v)
    }

    #[tokio::test]
    async fn correct_password_yields_working_channel() {
        let (h, v) = run("K7F92Q", "K7F92Q", false).await;
        let (mut hc, mut vc) = (h.ok().unwrap().channel, v.unwrap());
        assert_eq!(hc.open(&vc.seal(b"ping")).unwrap(), b"ping");
    }

    #[tokio::test]
    async fn wrong_password_fails_on_both_sides() {
        let (h, v) = run("K7F92Q", "AAAAAA", false).await;
        assert!(matches!(h, Err(HostFailure::BadPassword { .. })));
        assert_eq!(v.err(), Some(ViewerFailure::WrongPassword));
    }

    #[tokio::test]
    async fn spoofed_viewer_identity_is_refused() {
        let (h, _v) = run("K7F92Q", "K7F92Q", true).await;
        assert!(matches!(h, Err(HostFailure::Protocol)));
    }

    #[tokio::test]
    async fn garbage_first_frame_is_a_protocol_error() {
        let (vl, mut hl) = memory_pair(LinkKind::Direct);
        vl.tx.send(vec![0xff; 50]).await.unwrap();
        let p = HostParams {
            session: generate_session_id(),
            host_id: "2",
            expected_viewer_id: "1",
        };
        assert!(matches!(
            host_handshake(&mut hl, &p, |_, _| PasswordLookup::Disabled).await,
            Err(HostFailure::Protocol)
        ));
    }
}
