mod support;

use remotex_common::peer::{AuthKind, InputEvent, MouseButton, Permissions};
use remotex_session::{
    host::HostCmd,
    link::LinkKind,
    testing::SharedClipboard,
    types::{EndReason, Event, FailReason, HostNotice, TransferStatus, ViewerState},
    viewer::ViewerCmd,
};
use std::time::Duration;
use support::{clipboard, node, Node, NodeOpts, TestServer};

/// Connects `viewer` to `host`, has the host accept, and returns the viewer session number.
async fn establish(host: &Node, viewer: &Node, grant: Permissions) -> u64 {
    let hm = host.rec.mark();
    let session = viewer
        .engine
        .connect(&host.id, &host.password, AuthKind::Temporary, Permissions::ALL)
        .unwrap();
    let request = host
        .rec
        .wait_from(hm, 10, |e| {
            if let Event::Incoming(r) = e {
                Some(r.clone())
            } else {
                None
            }
        })
        .await;
    assert_eq!(request.viewer_id, viewer.id);
    host.engine.respond_incoming(request.request_id, Some(grant));
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

async fn eventually(secs: u64, mut f: impl FnMut() -> bool) {
    let end = tokio::time::Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(tokio::time::Instant::now() < end, "condition not met in time");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

fn failure(e: &Event) -> Option<FailReason> {
    if let Event::Viewer {
        state: ViewerState::Failed { reason },
        ..
    } = e
    {
        Some(*reason)
    } else {
        None
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_session_over_relay() {
    let server = TestServer::start().await;
    let host = node(&server, "Host PC", NodeOpts::default()).await;
    let viewer = node(&server, "Viewer PC", NodeOpts::default()).await;
    assert_ne!(host.id, viewer.id);

    let session = establish(&host, &viewer, Permissions::ALL).await;

    let info = viewer
        .rec
        .wait(5, |e| {
            if let Event::ViewerReady { info, .. } = e {
                Some(info.clone())
            } else {
                None
            }
        })
        .await;
    assert_eq!(info.link.kind, LinkKind::Relay);
    assert_eq!(info.displays.len(), 2);
    assert_eq!(info.host_name, "Host PC");

    eventually(5, || viewer.rec.frames() > 5).await;
    assert!(viewer.rec.last_video.lock().unwrap().is_some());

    // Keyboard and mouse reach the host's injector.
    viewer.engine.viewer_command(
        session,
        ViewerCmd::Input(InputEvent::Key {
            scancode: 0x1e,
            extended: false,
            down: true,
        }),
    );
    viewer.engine.viewer_command(
        session,
        ViewerCmd::Input(InputEvent::MouseButton {
            button: MouseButton::Left,
            down: true,
        }),
    );
    eventually(5, || host.env.input.lock().unwrap().len() >= 2).await;

    // Switching display without reconnecting.
    viewer.engine.viewer_command(session, ViewerCmd::SelectDisplay(1));
    viewer
        .rec
        .wait(5, |e| {
            matches!(e, Event::ViewerDisplays { active: 1, .. }).then_some(())
        })
        .await;
    eventually(5, || {
        viewer
            .rec
            .last_video
            .lock()
            .unwrap()
            .is_some_and(|(w, _, _)| w <= 320)
    })
    .await;
    viewer.engine.viewer_command(
        session,
        ViewerCmd::Input(InputEvent::MouseMove { x: 100, y: 100 }),
    );
    eventually(5, || {
        host.env
            .input
            .lock()
            .unwrap()
            .iter()
            .any(|(e, d)| matches!(e, InputEvent::MouseMove { .. }) && *d == 1)
    })
    .await;

    // Clipboard both directions without echo loops.
    SharedClipboard::user_copy(clipboard(&viewer), "مرحبا from viewer");
    eventually(5, || {
        SharedClipboard::text(clipboard(&host)).as_deref() == Some("مرحبا from viewer")
    })
    .await;
    SharedClipboard::user_copy(clipboard(&host), "reply from host");
    eventually(5, || {
        SharedClipboard::text(clipboard(&viewer)).as_deref() == Some("reply from host")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        SharedClipboard::text(clipboard(&host)).as_deref(),
        Some("reply from host"),
        "no echo overwrite"
    );

    // Chat.
    viewer
        .engine
        .viewer_command(session, ViewerCmd::Chat("hello host".into()));
    host.rec
        .wait(5, |e| {
            matches!(e, Event::Chat { text, mine: false, .. } if text == "hello host").then_some(())
        })
        .await;
    host.engine.host_command(HostCmd::Chat("hi viewer".into()));
    viewer
        .rec
        .wait(5, |e| {
            matches!(e, Event::Chat { text, mine: false, .. } if text == "hi viewer").then_some(())
        })
        .await;

    // File upload with verification.
    let src = tempfile::tempdir().unwrap();
    let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(src.path().join("report.bin"), &payload).unwrap();
    viewer
        .engine
        .viewer_command(session, ViewerCmd::SendFiles(vec![src.path().join("report.bin")]));
    viewer
        .rec
        .wait(10, |e| {
            matches!(e, Event::Transfer { info, .. } if info.status == TransferStatus::Completed)
                .then_some(())
        })
        .await;
    assert_eq!(
        std::fs::read(host.downloads.path().join("report.bin")).unwrap(),
        payload
    );

    // Host revokes the mouse: takes effect immediately and the viewer is told.
    let before = host.env.input.lock().unwrap().len();
    host.engine.host_command(HostCmd::SetPermissions(Permissions {
        mouse: false,
        ..Permissions::ALL
    }));
    viewer
        .rec
        .wait(5, |e| {
            matches!(e, Event::ViewerPermissions { permissions, .. } if !permissions.mouse).then_some(())
        })
        .await;
    viewer.engine.viewer_command(
        session,
        ViewerCmd::Input(InputEvent::MouseButton {
            button: MouseButton::Right,
            down: true,
        }),
    );
    viewer.engine.viewer_command(
        session,
        ViewerCmd::Input(InputEvent::Key {
            scancode: 0x30,
            extended: false,
            down: true,
        }),
    );
    eventually(5, || {
        host.env
            .input
            .lock()
            .unwrap()
            .iter()
            .skip(before)
            .any(|(e, _)| matches!(e, InputEvent::Key { scancode: 0x30, .. }))
    })
    .await;
    assert!(
        !host
            .env
            .input
            .lock()
            .unwrap()
            .iter()
            .skip(before)
            .any(|(e, _)| matches!(
                e,
                InputEvent::MouseButton {
                    button: MouseButton::Right,
                    ..
                }
            )),
        "mouse input must be dropped after revocation"
    );

    // Clean disconnect ends the host session and rotates the temporary password.
    let old_pw = host.password.clone();
    viewer.engine.viewer_command(session, ViewerCmd::Disconnect);
    host.rec
        .wait(5, |e| {
            matches!(
                e,
                Event::HostEnded {
                    reason: EndReason::PeerClosed
                }
            )
            .then_some(())
        })
        .await;
    eventually(5, || host.engine.temp_password() != old_pw).await;
    assert!(
        host.env
            .input
            .lock()
            .unwrap()
            .iter()
            .any(|(e, _)| matches!(e, InputEvent::ReleaseAll)),
        "held input released on exit"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lan_peers_use_a_direct_link() {
    let server = TestServer::start().await;
    let host = node(&server, "Host", NodeOpts { direct: true }).await;
    let viewer = node(&server, "Viewer", NodeOpts { direct: true }).await;
    establish(&host, &viewer, Permissions::ALL).await;
    let info = viewer
        .rec
        .wait(5, |e| {
            if let Event::ViewerReady { info, .. } = e {
                Some(info.clone())
            } else {
                None
            }
        })
        .await;
    assert_eq!(info.link.kind, LinkKind::Direct);
    eventually(5, || viewer.rec.frames() > 3).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_can_decline_and_viewer_learns_why() {
    let server = TestServer::start().await;
    let host = node(&server, "Host", NodeOpts::default()).await;
    let viewer = node(&server, "Viewer", NodeOpts::default()).await;
    let hm = host.rec.mark();
    viewer
        .engine
        .connect(&host.id, &host.password, AuthKind::Temporary, Permissions::ALL)
        .unwrap();
    let req = host
        .rec
        .wait_from(hm, 10, |e| {
            if let Event::Incoming(r) = e {
                Some(r.request_id)
            } else {
                None
            }
        })
        .await;
    host.engine.respond_incoming(req, None);
    assert_eq!(viewer.rec.wait(10, failure).await, FailReason::Declined);
    assert!(!host.engine.host_active());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_password_is_refused_then_locked_out() {
    let server = TestServer::start().await;
    let host = node(&server, "Host", NodeOpts::default()).await;
    let viewer = node(&server, "Viewer", NodeOpts::default()).await;
    let hm = host.rec.mark();
    for attempt in 0..5 {
        let vm = viewer.rec.mark();
        viewer
            .engine
            .connect(&host.id, "AAAAAA", AuthKind::Temporary, Permissions::ALL)
            .unwrap();
        let reason = viewer.rec.wait_from(vm, 10, failure).await;
        assert!(
            matches!(reason, FailReason::WrongPassword | FailReason::Banned),
            "attempt {attempt}: {reason:?}"
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    host.rec
        .wait_from(hm, 5, |e| {
            matches!(
                e,
                Event::HostNotice {
                    notice: HostNotice::LockedOut
                }
            )
            .then_some(())
        })
        .await;

    // Even the right password is now refused for this viewer, and no prompt reaches the host.
    let vm = viewer.rec.mark();
    let hm = host.rec.mark();
    viewer
        .engine
        .connect(&host.id, &host.password, AuthKind::Temporary, Permissions::ALL)
        .unwrap();
    let reason = viewer.rec.wait_from(vm, 10, failure).await;
    assert!(
        matches!(reason, FailReason::LockedOut | FailReason::Banned),
        "{reason:?}"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !host.rec.any_from(hm, |e| matches!(e, Event::Incoming(_))),
        "a locked-out viewer must not raise an approval dialog"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_or_invalid_ids_are_reported() {
    let server = TestServer::start().await;
    let viewer = node(&server, "Viewer", NodeOpts::default()).await;
    viewer
        .engine
        .connect("123456789", "AAAAAA", AuthKind::Temporary, Permissions::ALL)
        .unwrap();
    assert_eq!(viewer.rec.wait(10, failure).await, FailReason::Offline);
    assert_eq!(
        viewer
            .engine
            .connect("12345", "x", AuthKind::Temporary, Permissions::ALL)
            .err(),
        Some(remotex_common::signaling::ConnectFailure::InvalidId)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropped_link_reconnects_without_a_new_prompt() {
    let server = TestServer::start().await;
    let host = node(&server, "Host", NodeOpts::default()).await;
    let viewer = node(&server, "Viewer", NodeOpts::default()).await;
    let session = establish(&host, &viewer, Permissions::ALL).await;
    eventually(5, || viewer.rec.frames() > 2).await;

    let vm = viewer.rec.mark();
    host.engine.sever_links();
    viewer.engine.sever_links();
    viewer
        .rec
        .wait_from(vm, 10, |e| {
            matches!(
                e,
                Event::Viewer {
                    state: ViewerState::Reconnecting { .. },
                    ..
                }
            )
            .then_some(())
        })
        .await;
    viewer
        .rec
        .wait_from(vm, 20, |e| {
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

    let frames = viewer.rec.frames();
    eventually(5, || viewer.rec.frames() > frames + 2).await;
    viewer
        .engine
        .viewer_command(session, ViewerCmd::Chat("still here".into()));
    host.rec
        .wait(5, |e| {
            matches!(e, Event::Chat { text, .. } if text == "still here").then_some(())
        })
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unattended_access_requires_explicit_authorization() {
    let server = TestServer::start().await;
    let host = node(&server, "Host", NodeOpts::default()).await;
    let viewer = node(&server, "Viewer", NodeOpts::default()).await;

    // Disabled by default.
    viewer
        .engine
        .connect(&host.id, "correct-horse", AuthKind::Permanent, Permissions::ALL)
        .unwrap();
    assert_eq!(viewer.rec.wait(10, failure).await, FailReason::UnattendedDisabled);

    // Enabled, but this device is not authorised.
    host.engine.set_unattended(
        true,
        Some("correct-horse".into()),
        vec!["999999999".into()],
        false,
    );
    let vm = viewer.rec.mark();
    viewer
        .engine
        .connect(&host.id, "correct-horse", AuthKind::Permanent, Permissions::ALL)
        .unwrap();
    assert_eq!(
        viewer.rec.wait_from(vm, 10, failure).await,
        FailReason::UnattendedDisabled
    );

    // Authorised: connects with no prompt on the host.
    host.engine
        .set_unattended(true, Some("correct-horse".into()), vec![viewer.id.clone()], false);
    let hm = host.rec.mark();
    let vm = viewer.rec.mark();
    viewer
        .engine
        .connect(&host.id, "correct-horse", AuthKind::Permanent, Permissions::ALL)
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
    host.rec
        .wait_from(hm, 5, |e| matches!(e, Event::HostStarted { .. }).then_some(()))
        .await;
}

/// UI command threads have no Tokio context; starting a connection from one must not panic.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connect_works_from_a_plain_thread() {
    let server = TestServer::start().await;
    let host = node(&server, "Host", NodeOpts::default()).await;
    let viewer = node(&server, "Viewer", NodeOpts::default()).await;
    let engine = viewer.engine.clone();
    let (id, pw) = (host.id.clone(), host.password.clone());
    let session = std::thread::spawn(move || engine.connect(&id, &pw, AuthKind::Temporary, Permissions::ALL))
        .join()
        .expect("no panic")
        .unwrap();
    assert!(session > 0);
    let req = host
        .rec
        .wait(10, |e| {
            if let Event::Incoming(r) = e {
                Some(r.request_id)
            } else {
                None
            }
        })
        .await;
    host.engine.respond_incoming(req, Some(Permissions::ALL));
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
}

/// The service going away must not need an application restart: the engine reports the outage,
/// keeps its identity, retries on its own and comes back online when the service returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_recovers_by_itself_after_the_service_restarts() {
    use remotex_session::types::ServerState;
    let mut server = TestServer::start().await;
    let node = node(&server, "Resilient", NodeOpts::default()).await;
    let id_before = node.id.clone();

    let mark = node.rec.mark();
    server.stop_server().await;
    let outage = node
        .rec
        .wait_from(mark, 30, |e| match e {
            Event::Server(
                s @ (ServerState::Reconnecting { .. }
                | ServerState::ServiceUnavailable
                | ServerState::NoInternet),
            ) => Some(s.clone()),
            _ => None,
        })
        .await;
    eprintln!("outage state reported: {outage:?}");

    // After repeated failed retries the outage is reported as unavailable (not "reconnecting" forever).
    node.rec
        .wait_from(mark, 30, |e| {
            matches!(
                e,
                Event::Server(ServerState::ServiceUnavailable | ServerState::NoInternet)
            )
            .then_some(())
        })
        .await;

    let mark = node.rec.mark();
    server.start_again().await;
    node.rec
        .wait_from(mark, 60, |e| {
            matches!(e, Event::Server(ServerState::Online)).then_some(())
        })
        .await;
    let id_after = node
        .rec
        .wait_from(mark, 5, |e| {
            if let Event::Identity { id } = e {
                Some(id.clone())
            } else {
                None
            }
        })
        .await;
    assert_eq!(
        id_before, id_after,
        "the registered identity must survive a service restart"
    );
}
