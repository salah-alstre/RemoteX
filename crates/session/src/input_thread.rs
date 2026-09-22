//! Host-side input execution on its own thread.
//!
//! Input events are delivered here straight from the network reader ([`secure::spawn`]'s input route),
//! so nothing the session is otherwise busy with (clipboard, file transfers, statistics) can delay them.
//! Every wake-up applies the whole backlog in order, except that runs of mouse movement collapse into
//! the newest position: the pointer follows the latest state instead of replaying history. Clicks, keys,
//! wheel events and `ReleaseAll` are never dropped.

use crate::{
    env::{ElevatedInput, InputSink},
    secure::{InboundInput, SecureTx},
};
use remotex_common::peer::{DisplayInfo, InputEvent, Msg, Permissions};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc as std_mpsc, Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// State the session loop shares with the input thread.
pub struct InputShared {
    mouse: AtomicBool,
    keyboard: AtomicBool,
    displays: Mutex<Vec<DisplayInfo>>,
    active: AtomicU32,
    /// Movements dropped because a newer position was already waiting or had been applied.
    pub stale_moves: AtomicU64,
    /// Smoothed microseconds an event waited between arriving and being applied.
    pub queue_us: AtomicU32,
    /// Smoothed microseconds the operating system took to accept an event.
    pub apply_us: AtomicU32,
    /// While set, input goes through the elevated-control service instead of the ordinary injector.
    elevated: Mutex<Option<Box<dyn ElevatedInput>>>,
    /// Set when the service ended the grant underneath us (revoked, lapsed or unreachable).
    elevated_lost: AtomicBool,
}

impl InputShared {
    pub fn new(perms: Permissions, displays: Vec<DisplayInfo>, active: u32) -> Arc<Self> {
        Arc::new(Self {
            mouse: AtomicBool::new(perms.mouse),
            keyboard: AtomicBool::new(perms.keyboard),
            displays: Mutex::new(displays),
            active: AtomicU32::new(active),
            stale_moves: AtomicU64::new(0),
            queue_us: AtomicU32::new(0),
            apply_us: AtomicU32::new(0),
            elevated: Mutex::new(None),
            elevated_lost: AtomicBool::new(false),
        })
    }

    /// Switches input to (or away from) the elevated path.
    pub fn set_elevated(&self, e: Option<Box<dyn ElevatedInput>>) {
        *self.elevated.lock().unwrap_or_else(|p| p.into_inner()) = e;
        self.elevated_lost.store(false, Ordering::SeqCst);
    }

    /// True (once) if the elevated grant disappeared without the session asking for that.
    pub fn take_elevated_lost(&self) -> bool {
        self.elevated_lost.swap(false, Ordering::SeqCst)
    }

    pub fn set_permissions(&self, p: Permissions) {
        self.mouse.store(p.mouse, Ordering::SeqCst);
        self.keyboard.store(p.keyboard, Ordering::SeqCst);
    }

    pub fn set_displays(&self, displays: Vec<DisplayInfo>, active: u32) {
        if let Ok(mut d) = self.displays.lock() {
            *d = displays;
        }
        self.active.store(active, Ordering::SeqCst);
    }

    pub fn set_active(&self, active: u32) {
        self.active.store(active, Ordering::SeqCst);
    }
}

fn is_move(e: &InputEvent) -> bool {
    matches!(e, InputEvent::MouseMove { .. })
}

fn is_mouse(e: &InputEvent) -> bool {
    matches!(
        e,
        InputEvent::MouseMove { .. } | InputEvent::MouseButton { .. } | InputEvent::Wheel { .. }
    )
}

fn is_keyboard(e: &InputEvent) -> bool {
    matches!(e, InputEvent::Key { .. } | InputEvent::Text(_))
}

/// True if `a` is older than `b` in wrapping 32-bit sequence space.
fn seq_before(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

fn smooth(cell: &AtomicU32, sample_us: u32) {
    let old = cell.load(Ordering::Relaxed);
    cell.store(
        if old == 0 {
            sample_us
        } else {
            (old * 7 + sample_us) / 8
        },
        Ordering::Relaxed,
    );
}

/// Collapses runs of consecutive mouse movements into the last one. Returns how many were dropped.
pub fn coalesce_moves(items: Vec<InboundInput>) -> (Vec<InboundInput>, u64) {
    let mut out: Vec<InboundInput> = Vec::with_capacity(items.len());
    let mut dropped = 0;
    for it in items {
        if is_move(&it.ev) && out.last().is_some_and(|p| is_move(&p.ev)) {
            *out.last_mut().expect("checked") = it;
            dropped += 1;
        } else {
            out.push(it);
        }
    }
    (out, dropped)
}

pub struct InputThread {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl InputThread {
    /// Stops the thread after it releases every held key and button.
    pub fn finish(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for InputThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

pub fn spawn(
    mut sink: Box<dyn InputSink>,
    shared: Arc<InputShared>,
    rx: std_mpsc::Receiver<InboundInput>,
    tx: SecureTx,
) -> InputThread {
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let join = std::thread::Builder::new()
        .name("input".into())
        .spawn(move || {
            elevate_thread_priority();
            let mut last_move_seq = 0u32;
            let mut last_ack = Instant::now() - Duration::from_secs(1);
            loop {
                let first = match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(i) => i,
                    Err(std_mpsc::RecvTimeoutError::Timeout) => {
                        if stop2.load(Ordering::SeqCst) {
                            break;
                        }
                        continue;
                    }
                    Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let mut batch = vec![first];
                while let Ok(i) = rx.try_recv() {
                    batch.push(i);
                }
                let (batch, dropped) = coalesce_moves(batch);
                if dropped > 0 {
                    shared.stale_moves.fetch_add(dropped, Ordering::Relaxed);
                }
                let mut newest: Option<(u32, u32, u32)> = None;
                for it in batch {
                    if is_move(&it.ev) {
                        if seq_before(it.seq, last_move_seq) {
                            shared.stale_moves.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        last_move_seq = it.seq;
                    }
                    let allowed = match &it.ev {
                        e if is_mouse(e) => shared.mouse.load(Ordering::SeqCst),
                        e if is_keyboard(e) => shared.keyboard.load(Ordering::SeqCst),
                        InputEvent::ReleaseAll => true,
                        _ => false,
                    };
                    let queue_us = it.arrived.elapsed().as_micros().min(u32::MAX as u128) as u32;
                    let apply_us = if allowed {
                        let display = shared
                            .displays
                            .lock()
                            .ok()
                            .and_then(|d| d.get(shared.active.load(Ordering::SeqCst) as usize).cloned());
                        let mut elevated = shared.elevated.lock().unwrap_or_else(|p| p.into_inner());
                        if elevated.as_ref().is_some_and(|e| !e.alive()) {
                            *elevated = None;
                            shared.elevated_lost.store(true, Ordering::SeqCst);
                        }
                        let t = Instant::now();
                        match (&it.ev, display, elevated.as_mut()) {
                            (InputEvent::ReleaseAll, d, e) => {
                                if let (Some(e), Some(d)) = (e, d) {
                                    e.apply(&InputEvent::ReleaseAll, &d);
                                }
                                sink.release_all();
                            }
                            (ev, Some(d), Some(e)) => e.apply(ev, &d),
                            (ev, Some(d), None) => sink.apply(ev, &d),
                            _ => {}
                        }
                        t.elapsed().as_micros() as u32
                    } else {
                        0
                    };
                    if it.seq != 0 {
                        tx.counters.applied_input_seq.store(it.seq, Ordering::Relaxed);
                        smooth(&shared.queue_us, queue_us);
                        smooth(&shared.apply_us, apply_us);
                        newest = Some((it.seq, queue_us, apply_us));
                    }
                }
                if let Some((seq, queue_us, apply_us)) = newest {
                    if last_ack.elapsed() >= Duration::from_millis(15) {
                        last_ack = Instant::now();
                        tx.send(Msg::InputAck {
                            seq,
                            queue_us,
                            apply_us,
                        });
                    }
                }
            }
            // Session over, connection lost or permission revoked: never leave a key or button held.
            sink.release_all();
            if let (Some(e), Some(d)) = (
                shared.elevated.lock().unwrap_or_else(|p| p.into_inner()).as_mut(),
                shared.displays.lock().ok().and_then(|d| d.first().cloned()),
            ) {
                e.apply(&InputEvent::ReleaseAll, &d);
            }
        })
        .expect("spawn input thread");
    InputThread {
        stop,
        join: Some(join),
    }
}

#[cfg(windows)]
fn elevate_thread_priority() {
    use windows_sys_priority::*;
    // SAFETY: adjusts the priority of the calling thread only.
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
    }
}

#[cfg(not(windows))]
fn elevate_thread_priority() {}

#[cfg(windows)]
mod windows_sys_priority {
    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetCurrentThread() -> isize;
        pub fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    }
    pub const THREAD_PRIORITY_ABOVE_NORMAL: i32 = 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use remotex_common::peer::MouseButton;

    fn item(seq: u32, ev: InputEvent) -> InboundInput {
        InboundInput {
            seq,
            ev,
            arrived: Instant::now(),
        }
    }

    #[test]
    fn stale_movement_is_dropped_but_clicks_are_not() {
        let mut v = Vec::new();
        for i in 0..1000u16 {
            v.push(item(i as u32 + 1, InputEvent::MouseMove { x: i, y: i }));
        }
        v.push(item(
            2000,
            InputEvent::MouseButton {
                button: MouseButton::Left,
                down: true,
            },
        ));
        v.push(item(2001, InputEvent::MouseMove { x: 5, y: 5 }));
        v.push(item(
            2002,
            InputEvent::Key {
                scancode: 0x1e,
                extended: false,
                down: true,
            },
        ));
        v.push(item(
            2003,
            InputEvent::Key {
                scancode: 0x1e,
                extended: false,
                down: false,
            },
        ));
        let (out, dropped) = coalesce_moves(v);
        assert_eq!(dropped, 999);
        assert_eq!(out.len(), 5);
        assert!(matches!(out[0].ev, InputEvent::MouseMove { x: 999, .. }));
        assert!(matches!(out[1].ev, InputEvent::MouseButton { down: true, .. }));
        assert!(matches!(out[4].ev, InputEvent::Key { down: false, .. }));
    }

    #[test]
    fn sequence_comparison_handles_wraparound() {
        assert!(seq_before(1, 2));
        assert!(!seq_before(2, 1));
        assert!(seq_before(u32::MAX, 3));
        assert!(!seq_before(3, u32::MAX));
    }
}
