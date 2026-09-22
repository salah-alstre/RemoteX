//! Input stress test: a flood of mouse movement (far faster than the host can apply it) interleaved with
//! clicks and keys. Checks the guarantees the input path is built on:
//!   * the latest pointer position always wins and older positions are never replayed after newer ones,
//!   * every click and key event arrives, in order, exactly once,
//!   * the backlog drains promptly (queues do not grow without bound),
//!   * memory stays bounded while the flood is running.

mod support;

use remotex_common::peer::{AuthKind, DisplayInfo, InputEvent, MouseButton, Permissions};
use remotex_session::{
    env::InputSink,
    testing::test_env,
    types::{Event, ViewerState},
    viewer::ViewerCmd,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use support::{node_with_env_url, NodeOpts, TestServer};

#[derive(Debug, Clone, PartialEq)]
enum Seen {
    Move(u16),
    Down,
    Up,
    Key(u16, bool),
}

struct SlowInput {
    log: Arc<Mutex<Vec<Seen>>>,
    /// Simulates an operating system that needs this long per event.
    per_event: Duration,
}

impl InputSink for SlowInput {
    fn apply(&mut self, e: &InputEvent, _d: &DisplayInfo) {
        std::thread::sleep(self.per_event);
        let s = match e {
            InputEvent::MouseMove { x, .. } => Seen::Move(*x),
            InputEvent::MouseButton { down: true, .. } => Seen::Down,
            InputEvent::MouseButton { down: false, .. } => Seen::Up,
            InputEvent::Key { scancode, down, .. } => Seen::Key(*scancode, *down),
            _ => return,
        };
        self.log.lock().unwrap().push(s);
    }
    fn release_all(&mut self) {}
}

/// Resident memory of this process in bytes (Windows working set), or 0 elsewhere.
#[cfg(windows)]
fn rss() -> usize {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_faults: u32,
        peak: usize,
        working_set: usize,
        rest: [usize; 6],
    }
    #[link(name = "psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(h: isize, c: *mut Counters, cb: u32) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
    }
    let mut c = Counters {
        cb: std::mem::size_of::<Counters>() as u32,
        ..Default::default()
    };
    // SAFETY: plain FFI call with a correctly sized, initialised struct.
    unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    c.working_set
}

#[cfg(not(windows))]
fn rss() -> usize {
    0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flood_of_moves_keeps_clicks_ordered_and_latest_position_wins() {
    let server = TestServer::start().await;
    let log: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let mut host_env = test_env();
    let l2 = log.clone();
    host_env.env.input = Arc::new(move || {
        Box::new(SlowInput {
            log: l2.clone(),
            per_event: Duration::from_micros(400),
        })
    });
    let host = node_with_env_url(&server.url, "Host", NodeOpts::default(), host_env).await;
    let viewer = node_with_env_url(&server.url, "Viewer", NodeOpts::default(), test_env()).await;

    let hm = host.rec.mark();
    let session = viewer
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
    host.engine.respond_incoming(req, Some(Permissions::ALL));
    viewer
        .rec
        .wait(15, |e| {
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
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mem_before = rss();
    let total_moves = 300_000u32;
    let mut clicks = 0u32;
    let mut last_x = 0u16;
    let started = Instant::now();
    for i in 1..=total_moves {
        last_x = (i % 65_000) as u16 + 1;
        viewer.engine.viewer_command(
            session,
            ViewerCmd::Input(InputEvent::MouseMove { x: last_x, y: 1 }),
        );
        if i % 2_000 == 0 {
            clicks += 1;
            viewer.engine.viewer_command(
                session,
                ViewerCmd::Input(InputEvent::MouseButton {
                    button: MouseButton::Left,
                    down: true,
                }),
            );
            viewer.engine.viewer_command(
                session,
                ViewerCmd::Input(InputEvent::MouseButton {
                    button: MouseButton::Left,
                    down: false,
                }),
            );
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
                ViewerCmd::Input(InputEvent::Key {
                    scancode: 0x1e,
                    extended: false,
                    down: false,
                }),
            );
        }
    }
    let flood = started.elapsed();
    let mem_flood = rss();

    // The backlog must drain quickly once the flood stops.
    let drain_start = Instant::now();
    loop {
        let done = log.lock().unwrap().iter().rev().find_map(|s| match s {
            Seen::Move(x) => Some(*x == last_x),
            _ => None,
        });
        if done == Some(true) {
            break;
        }
        assert!(
            drain_start.elapsed() < Duration::from_secs(5),
            "latest position never arrived"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let drain = drain_start.elapsed();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let seen = log.lock().unwrap().clone();
    let moves = seen.iter().filter(|s| matches!(s, Seen::Move(_))).count();
    eprintln!(
        "sent {total_moves} moves in {flood:?}; host applied {moves}; drained in {drain:?}; memory {} MB -> {} MB",
        mem_before / 1_048_576,
        mem_flood / 1_048_576
    );

    // Newest position wins and nothing is replayed backwards.
    assert!(
        moves < total_moves as usize / 4,
        "stale positions were replayed ({moves} applied)"
    );
    assert!(
        matches!(seen.iter().rev().find(|s| matches!(s, Seen::Move(_))), Some(Seen::Move(x)) if *x == last_x)
    );

    // Clicks and keys: every one, in order.
    let discrete: Vec<&Seen> = seen.iter().filter(|s| !matches!(s, Seen::Move(_))).collect();
    let mut expected = Vec::new();
    for _ in 0..clicks {
        expected.extend([
            Seen::Down,
            Seen::Up,
            Seen::Key(0x1e, true),
            Seen::Key(0x1e, false),
        ]);
    }
    assert_eq!(
        discrete.len(),
        expected.len(),
        "clicks/keys were dropped or duplicated"
    );
    assert!(
        discrete.iter().zip(&expected).all(|(a, b)| *a == b),
        "clicks/keys were reordered"
    );

    // Bounded resources.
    assert!(drain < Duration::from_secs(2), "backlog took {drain:?} to drain");
    if mem_before > 0 {
        assert!(
            mem_flood.saturating_sub(mem_before) < 96 * 1_048_576,
            "memory grew by {} MB during the flood",
            mem_flood.saturating_sub(mem_before) / 1_048_576
        );
    }
}
