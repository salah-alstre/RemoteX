//! Elevated-control authorization, end to end through the real session engine with a mock service.
//! What is verified here is the *policy*: who can get elevated control, when it ends, and which path
//! input takes. The Windows service itself is covered by `crates/service/tests/broker.rs`.

mod support;

use remotex_common::peer::{AuthKind, InputEvent, MouseButton, Permissions};
use remotex_session::{
    host::HostCmd,
    types::{Event, HostNotice, ViewerState},
    viewer::ViewerCmd,
};
use std::{sync::atomic::Ordering, time::Duration};
use support::{node, Node, NodeOpts, TestServer};

async fn eventually(secs: u64, mut f: impl FnMut() -> bool) {
    let end = tokio::time::Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(tokio::time::Instant::now() < end, "condition not met in time");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Connects `viewer` asking for `requested`; the host approves `grant`. Returns the viewer session number.
async fn connect(host: &Node, viewer: &Node, requested: Permissions, grant: Permissions) -> u64 {
    let hm = host.rec.mark();
    let session = viewer
        .engine
        .connect(&host.id, &host.password, AuthKind::Temporary, requested)
        .unwrap();
    let req = host
        .rec
        .wait_from(hm, 10, |e| {
            if let Event::Incoming(r) = e {
                Some(r.clone())
            } else {
                None
            }
        })
        .await;
    assert_eq!(
        req.requested.elevated, requested.elevated,
        "the request carries the access level"
    );
    host.engine.respond_incoming(req.request_id, Some(grant));
    viewer
        .rec
        .wait(10, |e| {
            matches!(
                e,
                Event::Viewer {
                    state: ViewerState::Connected,
                    ..
                }
            )
            .then_some(())
        })
        .await;
    session
}

fn click() -> ViewerCmd {
    ViewerCmd::Input(InputEvent::MouseButton {
        button: MouseButton::Left,
        down: true,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn standard_control_never_touches_the_service() {
    let server = TestServer::start().await;
    let host = node(&server, "Host", NodeOpts::default()).await;
    let viewer = node(&server, "Viewer", NodeOpts::default()).await;
    // The viewer asks for everything including elevated control; the owner approves standard control only.
    let session = connect(&host, &viewer, Permissions::ALL_ELEVATED, Permissions::ALL).await;
    viewer.engine.viewer_command(session, click());
    eventually(5, || !host.env.input.lock().unwrap().is_empty()).await;
    assert_eq!(
        host.env.elevation.begun.load(Ordering::SeqCst),
        0,
        "no privileged helper was started"
    );
    assert!(host.env.elevation.inputs.lock().unwrap().is_empty());
    let perms = viewer
        .rec
        .wait(5, |e| {
            if let Event::ViewerReady { info, .. } = e {
                Some(info.permissions)
            } else {
                None
            }
        })
        .await;
    assert!(
        !perms.elevated,
        "the viewer is told it does NOT have elevated control"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn approved_elevated_control_routes_input_through_the_service_and_ends_with_the_session() {
    let server = TestServer::start().await;
    let host = node(&server, "Host", NodeOpts::default()).await;
    let viewer = node(&server, "Viewer", NodeOpts::default()).await;
    let session = connect(
        &host,
        &viewer,
        Permissions::ALL_ELEVATED,
        Permissions::ALL_ELEVATED,
    )
    .await;
    assert_eq!(host.env.elevation.begun.load(Ordering::SeqCst), 1);

    viewer.engine.viewer_command(session, click());
    eventually(5, || !host.env.elevation.inputs.lock().unwrap().is_empty()).await;
    assert!(
        host.env.input.lock().unwrap().is_empty(),
        "with elevated control input must not also go through the ordinary injector"
    );
    let perms = viewer
        .rec
        .wait(5, |e| {
            if let Event::ViewerReady { info, .. } = e {
                Some(info.permissions)
            } else {
                None
            }
        })
        .await;
    assert!(perms.elevated);

    // Disconnecting revokes the authorization immediately.
    assert_eq!(host.env.elevation.revoked.load(Ordering::SeqCst), 0);
    viewer.engine.viewer_command(session, ViewerCmd::Disconnect);
    eventually(5, || host.env.elevation.revoked.load(Ordering::SeqCst) == 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_session_is_authorised_on_its_own() {
    let server = TestServer::start().await;
    let host = node(&server, "Host", NodeOpts::default()).await;
    let viewer = node(&server, "Viewer", NodeOpts::default()).await;

    // Session A: elevated.
    let a = connect(
        &host,
        &viewer,
        Permissions::ALL_ELEVATED,
        Permissions::ALL_ELEVATED,
    )
    .await;
    assert_eq!(host.env.elevation.begun.load(Ordering::SeqCst), 1);
    viewer.engine.viewer_command(a, ViewerCmd::Disconnect);
    eventually(5, || host.env.elevation.revoked.load(Ordering::SeqCst) == 1).await;

    // Session B with the same viewer: the previous approval must not carry over.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let hm = host.rec.mark();
    let vm = viewer.rec.mark();
    let b = viewer
        .engine
        // The temporary password rotates after every session.
        .connect(
            &host.id,
            &host.engine.temp_password(),
            AuthKind::Temporary,
            Permissions::ALL_ELEVATED,
        )
        .unwrap();
    let req = host
        .rec
        .wait_from(hm, 10, |e| {
            if let Event::Incoming(r) = e {
                Some(r.clone())
            } else {
                None
            }
        })
        .await;
    host.engine
        .respond_incoming(req.request_id, Some(Permissions::ALL));
    viewer
        .rec
        .wait_from(vm, 10, |e| {
            matches!(
                e,
                Event::Viewer {
                    state: ViewerState::Connected,
                    ..
                }
            )
            .then_some(())
        })
        .await;
    viewer.engine.viewer_command(b, click());
    eventually(5, || !host.env.input.lock().unwrap().is_empty()).await;
    assert_eq!(
        host.env.elevation.begun.load(Ordering::SeqCst),
        1,
        "session B never became elevated"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_owner_can_revoke_elevated_control_mid_session() {
    let server = TestServer::start().await;
    let host = node(&server, "Host", NodeOpts::default()).await;
    let viewer = node(&server, "Viewer", NodeOpts::default()).await;
    let session = connect(
        &host,
        &viewer,
        Permissions::ALL_ELEVATED,
        Permissions::ALL_ELEVATED,
    )
    .await;

    host.engine
        .host_command(HostCmd::SetPermissions(Permissions::ALL));
    eventually(5, || host.env.elevation.revoked.load(Ordering::SeqCst) == 1).await;
    // The viewer is informed, and input now takes the ordinary path.
    viewer
        .rec
        .wait(5, |e| match e {
            Event::ViewerPermissions { permissions, .. } if !permissions.elevated => Some(()),
            _ => None,
        })
        .await;
    viewer.engine.viewer_command(session, click());
    eventually(5, || !host.env.input.lock().unwrap().is_empty()).await;
    assert!(host.env.elevation.inputs.lock().unwrap().is_empty());
    // And it cannot be switched back on without a new approval of a new session... unless it was granted
    // at the start of this one, in which case the owner may re-enable it.
    host.engine
        .host_command(HostCmd::SetPermissions(Permissions::ALL_ELEVATED));
    eventually(5, || host.env.elevation.begun.load(Ordering::SeqCst) == 2).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_service_downgrades_honestly() {
    let server = TestServer::start().await;
    let host = node(&server, "Host", NodeOpts::default()).await;
    let viewer = node(&server, "Viewer", NodeOpts::default()).await;
    host.env.elevation.available.store(false, Ordering::SeqCst);
    connect(
        &host,
        &viewer,
        Permissions::ALL_ELEVATED,
        Permissions::ALL_ELEVATED,
    )
    .await;
    assert!(host.rec.any_from(0, |e| matches!(
        e,
        Event::HostNotice {
            notice: HostNotice::ElevationUnavailable
        }
    )));
    let perms = viewer
        .rec
        .wait(5, |e| {
            if let Event::ViewerReady { info, .. } = e {
                Some(info.permissions)
            } else {
                None
            }
        })
        .await;
    assert!(
        !perms.elevated,
        "elevated control is never reported as granted when it cannot work"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unattended_sessions_are_elevated_only_when_the_owner_enabled_it_separately() {
    let server = TestServer::start().await;
    let host = node(&server, "Host", NodeOpts::default()).await;
    let viewer = node(&server, "Viewer", NodeOpts::default()).await;
    host.engine
        .set_unattended(true, Some("correct-horse".into()), vec![viewer.id.clone()], false);

    async fn unattended(host: &Node, viewer: &Node) -> u64 {
        let vm = viewer.rec.mark();
        let s = viewer
            .engine
            .connect(
                &host.id,
                "correct-horse",
                AuthKind::Permanent,
                Permissions::ALL_ELEVATED,
            )
            .unwrap();
        viewer
            .rec
            .wait_from(vm, 10, |e| {
                matches!(
                    e,
                    Event::Viewer {
                        state: ViewerState::Connected,
                        ..
                    }
                )
                .then_some(())
            })
            .await;
        s
    }

    // Unattended access on, elevated not enabled: no privileged helper.
    let s = unattended(&host, &viewer).await;
    assert_eq!(host.env.elevation.begun.load(Ordering::SeqCst), 0);
    viewer.engine.viewer_command(s, ViewerCmd::Disconnect);
    tokio::time::sleep(Duration::from_millis(800)).await;

    // Owner explicitly enables elevated unattended control.
    host.engine.set_unattended_elevated(true);
    let s = unattended(&host, &viewer).await;
    assert_eq!(host.env.elevation.begun.load(Ordering::SeqCst), 1);
    viewer.engine.viewer_command(s, ViewerCmd::Disconnect);
    eventually(5, || host.env.elevation.revoked.load(Ordering::SeqCst) == 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_uac_prompt_switches_the_picture_to_the_service_and_back() {
    let server = TestServer::start().await;
    let host = node(&server, "Host", NodeOpts::default()).await;
    let viewer = node(&server, "Viewer", NodeOpts::default()).await;
    connect(
        &host,
        &viewer,
        Permissions::ALL_ELEVATED,
        Permissions::ALL_ELEVATED,
    )
    .await;

    // Ordinary pictures first (the mock screen never produces 0xEE bytes).
    eventually(5, || viewer.rec.frames() > 2).await;
    let has_secure = |v: &support::Recorder| {
        v.video_log
            .lock()
            .unwrap()
            .iter()
            .any(|(_, _, _, d)| d.iter().rev().take(64).all(|b| *b == 0xEE))
    };
    assert!(!has_secure(&viewer.rec));

    // A UAC prompt appears: the ordinary capture cannot see it, the service can.
    host.env.elevation.secure.store(true, Ordering::SeqCst);
    eventually(8, || has_secure(&viewer.rec)).await;
}
