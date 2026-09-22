//! Exercises the service's broker end to end over real named pipes, with the real agent executable.
//!
//! Runs without administrator rights: the broker starts the agent as the current user
//! (`LaunchKind::SameUser`). The parts that need SYSTEM (a token duplicated into the console session,
//! the secure desktop) are covered by the manual/elevated test procedure in `docs/ELEVATED_CONTROL.md`.
//! Only harmless input is injected (`ReleaseAll`), so the test never moves the real mouse.

use remotex_common::peer::{DisplayInfo, InputEvent};
use remotex_elevate::{
    pipe, read_msg, write_msg, ConnRole, DenyReason, ServiceToUi, UiToService, PROTOCOL_VERSION,
};
use remotex_service::{
    broker::{Broker, BrokerConfig, LaunchKind},
    server,
};
use std::{
    fs::File,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

fn agent_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_remotex-service"))
}

fn display() -> DisplayInfo {
    DisplayInfo {
        index: 0,
        name: "test".into(),
        width: 640,
        height: 360,
        x: 0,
        y: 0,
        primary: true,
    }
}

struct Harness {
    name: String,
    stop: Arc<AtomicBool>,
    broker: Arc<Broker>,
}

impl Harness {
    fn start(id: &str, allowed: Vec<PathBuf>) -> Self {
        let name = format!(r"\\.\pipe\remotex-broker-test-{}-{id}", std::process::id());
        let broker = Broker::new(BrokerConfig {
            exe: agent_exe(),
            allowed_ui: allowed,
            launch: LaunchKind::SameUser,
            enforce_console_session: false,
            require_protected_install: false,
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (b, s, n) = (broker.clone(), stop.clone(), name.clone());
        std::thread::spawn(move || {
            let _ = server::serve(b, s, &n);
        });
        // Wait for the pipe to exist.
        let deadline = Instant::now() + Duration::from_secs(5);
        while pipe::connect(&name, Duration::from_millis(0)).is_err() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        Self { name, stop, broker }
    }

    fn client(&self) -> File {
        let mut c = pipe::connect(&self.name, Duration::from_secs(5)).expect("connect");
        write_msg(
            &mut c,
            &UiToService::Hello {
                version: PROTOCOL_VERSION,
            },
        )
        .unwrap();
        c
    }

    fn welcomed(&self) -> File {
        let mut c = self.client();
        assert!(matches!(
            read_msg::<_, ServiceToUi>(&mut c).unwrap(),
            ServiceToUi::Welcome { .. }
        ));
        c
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        server::wake(&self.name);
    }
}

fn ask(c: &mut File, m: &UiToService) -> ServiceToUi {
    write_msg(c, m).unwrap();
    read_msg(c).unwrap()
}

fn grant(c: &mut File) -> [u8; 16] {
    match ask(
        c,
        &UiToService::Grant {
            label: "test session".into(),
        },
    ) {
        ServiceToUi::Granted { token } => token,
        other => panic!("grant failed: {other:?}"),
    }
}

fn agent_processes() -> usize {
    // Count running agents by asking the OS for processes named like our binary that were started with
    // the `agent` argument; falls back to the whole image name if command lines are unavailable.
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_Process -Filter \"Name='remotex-service.exe'\" | Where-Object { $_.CommandLine -match ' agent ' }).Count",
        ])
        .output()
        .expect("powershell");
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
}

#[test]
fn unknown_callers_are_refused_before_any_message_is_read() {
    // The test process is not on the allow list, so the service must not even greet it.
    let h = Harness::start(
        "deny",
        vec![PathBuf::from(r"C:\Program Files\RemoteX\remotex.exe")],
    );
    let mut c = h.client();
    match read_msg::<_, ServiceToUi>(&mut c) {
        Ok(ServiceToUi::Denied(DenyReason::NotAuthorized)) => {}
        other => panic!("expected refusal, got {other:?}"),
    }
    assert!(!h.broker.has_grant());
}

#[test]
fn grant_lifecycle_input_capture_and_revocation() {
    let h = Harness::start("life", vec![std::env::current_exe().unwrap()]);
    let mut owner = h.welcomed();

    // Nothing works before a grant.
    assert!(matches!(
        ask(
            &mut owner,
            &UiToService::Capture {
                display: display(),
                out_w: 320,
                out_h: 180
            }
        ),
        ServiceToUi::Denied(DenyReason::NotGranted)
    ));

    let token = grant(&mut owner);
    assert!(h.broker.has_grant());

    // A second grant is refused while one is active.
    let mut other = h.welcomed();
    assert!(matches!(
        ask(
            &mut other,
            &UiToService::Grant {
                label: "second".into()
            }
        ),
        ServiceToUi::Denied(DenyReason::AlreadyGranted)
    ));
    // A wrong token cannot attach.
    assert!(matches!(
        ask(
            &mut other,
            &UiToService::Attach {
                token: [9; 16],
                role: ConnRole::Video
            }
        ),
        ServiceToUi::Denied(_)
    ));

    // The second connection can join with the real token (input and video use separate pipes).
    let mut video = h.welcomed();
    assert!(matches!(
        ask(
            &mut video,
            &UiToService::Attach {
                token,
                role: ConnRole::Video
            }
        ),
        ServiceToUi::Attached
    ));

    // Status through the agent: the ordinary desktop, so not secure.
    match ask(&mut video, &UiToService::Status) {
        ServiceToUi::Status {
            secure_desktop,
            agent_running,
        } => {
            assert!(agent_running);
            assert!(!secure_desktop, "the test session is on the default desktop");
        }
        other => panic!("{other:?}"),
    }
    // A real capture of the current desktop through the agent.
    match ask(
        &mut video,
        &UiToService::Capture {
            display: display(),
            out_w: 320,
            out_h: 180,
        },
    ) {
        ServiceToUi::Picture(p) => {
            assert_eq!((p.width, p.height), (320, 180));
            assert_eq!(p.nv12.len(), 320 * 180 * 3 / 2);
        }
        other => panic!("expected a picture, got {other:?}"),
    }
    // Harmless input reaches the agent without a reply.
    write_msg(
        &mut owner,
        &UiToService::Input {
            ev: InputEvent::ReleaseAll,
            display: display(),
        },
    )
    .unwrap();
    // Heartbeats are one-way as well.
    write_msg(&mut owner, &UiToService::Heartbeat).unwrap();
    assert!(agent_processes() >= 1, "the agent runs while the grant is active");

    // Revocation is immediate and total.
    assert!(matches!(
        ask(&mut owner, &UiToService::Revoke),
        ServiceToUi::Revoked
    ));
    assert!(!h.broker.has_grant());
    assert!(matches!(
        ask(
            &mut video,
            &UiToService::Capture {
                display: display(),
                out_w: 320,
                out_h: 180
            }
        ),
        ServiceToUi::Denied(_)
    ));
    let end = Instant::now() + Duration::from_secs(5);
    while agent_processes() > 0 && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(
        agent_processes(),
        0,
        "the privileged helper must not outlive the grant"
    );
}

#[test]
fn owner_disconnect_revokes_at_once_and_a_new_grant_works() {
    let h = Harness::start("owner", vec![std::env::current_exe().unwrap()]);
    {
        let mut owner = h.welcomed();
        grant(&mut owner);
        assert!(h.broker.has_grant());
        // Dropping the connection (app crash / disconnect) must revoke.
    }
    let end = Instant::now() + Duration::from_secs(5);
    while h.broker.has_grant() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!h.broker.has_grant(), "grant survived its owner");
    let mut again = h.welcomed();
    grant(&mut again);
    assert!(matches!(
        ask(&mut again, &UiToService::Revoke),
        ServiceToUi::Revoked
    ));
}

#[test]
fn grants_lapse_without_heartbeats() {
    let h = Harness::start("lapse", vec![std::env::current_exe().unwrap()]);
    let mut owner = h.welcomed();
    grant(&mut owner);
    // Keep the pipe open but stay silent: the dead-man switch must fire on its own.
    let end = Instant::now() + Duration::from_secs(remotex_elevate::GRANT_LAPSE_SECS + 6);
    while h.broker.has_grant() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(250));
    }
    assert!(!h.broker.has_grant(), "an idle grant must expire by itself");
}

#[test]
fn malformed_and_oversized_requests_are_rejected() {
    let h = Harness::start("bad", vec![std::env::current_exe().unwrap()]);
    let mut c = h.welcomed();
    let token = grant(&mut c);
    // Odd output size: fails validation and the connection is dropped, the grant ends with it.
    write_msg(
        &mut c,
        &UiToService::Capture {
            display: display(),
            out_w: 321,
            out_h: 180,
        },
    )
    .unwrap();
    assert!(read_msg::<_, ServiceToUi>(&mut c).is_err());
    let end = Instant::now() + Duration::from_secs(5);
    while h.broker.has_grant() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!h.broker.has_grant());
    let _ = token;
    // Garbage on a fresh connection never crashes the service.
    let mut g = h.client();
    use std::io::Write;
    let _ = g.write_all(&[0xff, 0xff, 0xff, 0x7f, 1, 2, 3]);
    drop(g);
    let mut ok = h.welcomed();
    assert!(matches!(
        ask(&mut ok, &UiToService::Status),
        ServiceToUi::Status { .. }
    ));
}
