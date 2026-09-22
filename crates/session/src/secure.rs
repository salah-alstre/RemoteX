//! Encrypted, prioritised message pipe on top of a [`Link`].
//!
//! Three send lanes share one ordered, encrypted stream: control (input, chat, pings, cursor) always
//! goes first, then video, then bulk data (file chunks, clipboard images). Three properties keep input
//! responsive:
//!
//! * **Fragmentation.** Video frames and file chunks are cut into small fragments and control messages
//!   are interleaved between them, so a 200 KB keyframe or a 256 KB file chunk never sits in front of a
//!   mouse click.
//! * **Late scheduling.** The writer reserves a slot on the (very shallow) link queue *before* choosing
//!   what to send, so the choice is made at the last possible moment and fresh input overtakes anything
//!   that is still waiting.
//! * **Coalescing.** Consecutive mouse movements waiting in the control lane collapse into the newest
//!   position. Clicks, keys and wheel events are never dropped or reordered.
//!
//! Frames are sealed in the order they are sent, so the AEAD counter nonce stays in step with the wire.

use crate::link::{Link, LinkKind};
use remotex_common::{
    peer::{InputEvent, Msg},
    wire::{self, MAX_FRAME},
};
use remotex_security::SecureChannel;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        mpsc as std_mpsc, Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch, Notify};

/// Largest unit handed to the transport for the video and bulk lanes.
pub const FRAGMENT: usize = 8 * 1024;

const LANE_CTL: u8 = 0;
const LANE_VIDEO: u8 = 1;
const LANE_BULK: u8 = 2;
const WHOLE: u8 = 0;
const FIRST: u8 = 1;
const MID: u8 = 2;
const LAST: u8 = 3;

const CLOCK_SLOTS: usize = 1024;

/// Send timestamps of recent input events, so acknowledgements and frames that carry a sequence number
/// can be turned into latency measurements without any clock synchronisation between the two machines.
pub struct InputClock {
    base: Instant,
    next: AtomicU32,
    /// `(microseconds since base) << 16 | low 16 bits of the sequence number`
    slots: Vec<AtomicU64>,
}

impl Default for InputClock {
    fn default() -> Self {
        Self {
            base: Instant::now(),
            next: AtomicU32::new(0),
            slots: (0..CLOCK_SLOTS).map(|_| AtomicU64::new(0)).collect(),
        }
    }
}

impl InputClock {
    fn now_us(&self) -> u64 {
        self.base.elapsed().as_micros() as u64
    }

    /// Allocates the next sequence number (never 0) and records when it was sent.
    pub fn stamp(&self) -> u32 {
        let mut seq = self.next.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        if seq == 0 {
            seq = self.next.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        }
        self.slots[seq as usize % CLOCK_SLOTS]
            .store((self.now_us() << 16) | (seq as u64 & 0xffff), Ordering::Relaxed);
        seq
    }

    /// Microseconds since `seq` was stamped, if it is still remembered.
    pub fn age_us(&self, seq: u32) -> Option<u64> {
        let v = self.slots[seq as usize % CLOCK_SLOTS].load(Ordering::Relaxed);
        (v != 0 && (v & 0xffff) == (seq as u64 & 0xffff)).then(|| self.now_us().saturating_sub(v >> 16))
    }
}

#[derive(Default)]
pub struct NetCounters {
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
    /// Time the most recent video frame spent queued plus being handed to the transport, microseconds.
    pub video_send_us: AtomicU64,
    pub video_dropped: AtomicU64,
    /// Mouse movements that were replaced by a newer position before they were sent.
    pub moves_coalesced: AtomicU64,
    /// Smoothed time an input event waited in the local control lane before being sent (microseconds).
    pub input_queue_us: AtomicU64,
    /// Smoothed input round trip (send -> host applied -> acknowledgement received), microseconds.
    pub input_rtt_us: AtomicU64,
    /// Smoothed time from sending an input event until a video frame reflecting it arrived (microseconds).
    pub motion_to_frame_us: AtomicU64,
    /// Newest input sequence number the host has applied (host side).
    pub applied_input_seq: AtomicU32,
    /// Cap on bulk (file) traffic in bytes per second; 0 means unlimited.
    pub bulk_rate_bps: AtomicU64,
    pub bulk_bytes_sent: AtomicU64,
    pub clock: InputClock,
}

fn ewma(cell: &AtomicU64, sample: u64) {
    let old = cell.load(Ordering::Relaxed);
    cell.store(
        if old == 0 { sample } else { (old * 7 + sample) / 8 },
        Ordering::Relaxed,
    );
}

struct Queued {
    msg: Msg,
    at: Instant,
}

/// An input event that arrived from the peer, delivered straight to the host's input thread.
pub struct InboundInput {
    pub seq: u32,
    pub ev: InputEvent,
    pub arrived: Instant,
}

#[derive(Clone)]
pub struct SecureTx {
    ctl: mpsc::UnboundedSender<Queued>,
    /// The newest mouse position that has not been queued yet. Positions replace each other here, so a
    /// stalled network can never make the backlog of movement grow: it is at most one entry.
    pending_move: Arc<std::sync::Mutex<Option<Queued>>>,
    wake: Arc<Notify>,
    video: mpsc::Sender<Queued>,
    bulk: mpsc::Sender<Queued>,
    pub counters: Arc<NetCounters>,
    pub kind: LinkKind,
}

impl SecureTx {
    /// Small latency-critical message. Returns false once the session is gone.
    pub fn send(&self, msg: Msg) -> bool {
        self.ctl
            .send(Queued {
                msg,
                at: Instant::now(),
            })
            .is_ok()
    }

    /// Sends an input event and returns the sequence number it was given.
    ///
    /// Mouse movement only updates the "newest position" slot. Any other event first moves that pending
    /// position into the ordered queue, so a click always follows the position it was made at.
    pub fn send_input(&self, ev: InputEvent) -> u32 {
        let seq = self.counters.clock.stamp();
        let mut item = Queued {
            msg: Msg::InputAt { seq, ev },
            at: Instant::now(),
        };
        let is_move = matches!(
            item.msg,
            Msg::InputAt {
                ev: InputEvent::MouseMove { .. },
                ..
            }
        );
        let mut slot = self.pending_move.lock().unwrap_or_else(|p| p.into_inner());
        if is_move {
            if let Some(old) = slot.take() {
                item.at = old.at; // the position has been waiting since the older one
                self.counters.moves_coalesced.fetch_add(1, Ordering::Relaxed);
            }
            *slot = Some(item);
            drop(slot);
            self.wake.notify_one();
        } else {
            if let Some(m) = slot.take() {
                let _ = self.ctl.send(m);
            }
            let _ = self.ctl.send(item);
            drop(slot);
        }
        seq
    }

    /// Video frame; returns false (and counts a drop) if the network is behind.
    pub fn try_send_video(&self, msg: Msg) -> bool {
        let ok = self
            .video
            .try_send(Queued {
                msg,
                at: Instant::now(),
            })
            .is_ok();
        if !ok {
            self.counters.video_dropped.fetch_add(1, Ordering::Relaxed);
        }
        ok
    }

    pub fn video_congested(&self) -> bool {
        self.video.capacity() == 0
    }

    /// Bulk data with backpressure.
    pub async fn send_bulk(&self, msg: Msg) -> bool {
        self.bulk
            .send(Queued {
                msg,
                at: Instant::now(),
            })
            .await
            .is_ok()
    }

    pub fn is_closed(&self) -> bool {
        self.ctl.is_closed()
    }
}

pub struct SecureRx {
    rx: mpsc::Receiver<Msg>,
}

impl SecureRx {
    pub async fn recv(&mut self) -> Option<Msg> {
        self.rx.recv().await
    }
}

/// Adds a control message, replacing a waiting mouse position with a newer one.
fn push_ctl(queue: &mut VecDeque<Queued>, item: Queued, counters: &NetCounters) {
    if matches!(
        item.msg,
        Msg::InputAt {
            ev: InputEvent::MouseMove { .. },
            ..
        }
    ) {
        if let Some(back) = queue.back_mut() {
            if matches!(
                back.msg,
                Msg::InputAt {
                    ev: InputEvent::MouseMove { .. },
                    ..
                }
            ) {
                // Keep the older timestamp so the recorded queue delay is the worst case for this position.
                let at = back.at;
                *back = Queued { at, ..item };
                counters.moves_coalesced.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    }
    queue.push_back(item);
}

/// A video/bulk message being sent fragment by fragment.
struct Outgoing {
    lane: u8,
    data: Vec<u8>,
    off: usize,
    at: Instant,
}

impl Outgoing {
    fn new(lane: u8, q: Queued) -> Option<Self> {
        Some(Self {
            lane,
            data: wire::encode(&q.msg).ok()?,
            off: 0,
            at: q.at,
        })
    }

    /// Next plaintext unit (header byte + payload) and whether the message is now complete.
    fn next_unit(&mut self) -> (Vec<u8>, bool) {
        let remaining = self.data.len() - self.off;
        let (flag, take) = if self.off == 0 && remaining <= FRAGMENT {
            (WHOLE, remaining)
        } else if self.off == 0 {
            (FIRST, FRAGMENT)
        } else if remaining <= FRAGMENT {
            (LAST, remaining)
        } else {
            (MID, FRAGMENT)
        };
        let mut unit = Vec::with_capacity(1 + take);
        unit.push((self.lane << 2) | flag);
        unit.extend_from_slice(&self.data[self.off..self.off + take]);
        self.off += take;
        (unit, self.off >= self.data.len())
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Bucket {
    /// Bytes available now under `rate` bytes/second (0 = unlimited); burst is a quarter second.
    fn available(&mut self, rate: u64) -> f64 {
        if rate == 0 {
            return f64::INFINITY;
        }
        let now = Instant::now();
        let cap = (rate as f64 * 0.25).max(FRAGMENT as f64);
        self.tokens = (self.tokens + now.duration_since(self.last).as_secs_f64() * rate as f64).min(cap);
        self.last = now;
        self.tokens
    }
}

/// Starts reader and writer tasks. `closed` flips to true when either direction ends.
/// If `input_route` is given, verified input events are delivered to it directly instead of through
/// [`SecureRx`], so nothing else the session is busy with can delay them.
pub fn spawn(
    link: Link,
    channel: SecureChannel,
    input_route: Option<std_mpsc::Sender<InboundInput>>,
) -> (SecureTx, SecureRx, watch::Receiver<bool>) {
    let kind = link.kind;
    let Link {
        tx: link_tx,
        rx: mut link_rx,
        ..
    } = link;
    let (mut sealer, mut opener) = channel.split();
    let counters = Arc::new(NetCounters::default());
    let (ctl_tx, mut ctl_rx) = mpsc::unbounded_channel::<Queued>();
    let (video_tx, mut video_rx) = mpsc::channel::<Queued>(2);
    let (bulk_tx, mut bulk_rx) = mpsc::channel::<Queued>(4);
    let (in_tx, in_rx) = mpsc::channel::<Msg>(256);
    let (closed_tx, closed_rx) = watch::channel(false);
    let pending_move: Arc<std::sync::Mutex<Option<Queued>>> = Arc::default();
    let wake = Arc::new(Notify::new());

    let c = counters.clone();
    let closed_w = closed_tx.clone();
    let (slot_w, wake_w) = (pending_move.clone(), wake.clone());
    let take_slot = move |ctl: &mut VecDeque<Queued>, c: &NetCounters| {
        if let Some(m) = slot_w.lock().unwrap_or_else(|p| p.into_inner()).take() {
            push_ctl(ctl, m, c);
        }
    };
    tokio::spawn(async move {
        let mut ctl: VecDeque<Queued> = VecDeque::new();
        let mut video: Option<Outgoing> = None;
        let mut bulk: Option<Outgoing> = None;
        let mut bucket = Bucket {
            tokens: 0.0,
            last: Instant::now(),
        };
        'outer: loop {
            // Gather whatever is ready right now.
            while let Ok(m) = ctl_rx.try_recv() {
                push_ctl(&mut ctl, m, &c);
            }
            take_slot(&mut ctl, &c);
            if video.is_none() {
                if let Ok(m) = video_rx.try_recv() {
                    video = Outgoing::new(LANE_VIDEO, m);
                }
            }
            if bulk.is_none() {
                if let Ok(m) = bulk_rx.try_recv() {
                    bulk = Outgoing::new(LANE_BULK, m);
                }
            }
            let rate = c.bulk_rate_bps.load(Ordering::Relaxed);
            let bulk_ok = bulk.is_some() && bucket.available(rate) >= 1.0;

            if ctl.is_empty() && video.is_none() && !bulk_ok {
                // Nothing to send: wait for the next arrival (or for bulk tokens to refill).
                let refill = bulk
                    .as_ref()
                    .map(|_| Duration::from_millis(5))
                    .unwrap_or(Duration::from_secs(3600));
                tokio::select! {
                    biased;
                    m = ctl_rx.recv() => match m {
                        Some(m) => push_ctl(&mut ctl, m, &c),
                        None => break 'outer,
                    },
                    m = video_rx.recv() => match m {
                        Some(m) => video = Outgoing::new(LANE_VIDEO, m),
                        None => break 'outer,
                    },
                    m = bulk_rx.recv(), if bulk.is_none() => match m {
                        Some(m) => bulk = Outgoing::new(LANE_BULK, m),
                        None => break 'outer,
                    },
                    _ = wake_w.notified() => {}
                    _ = tokio::time::sleep(refill) => {}
                }
                continue;
            }

            // Reserve a slot on the link *before* deciding what to send: whatever is waiting when the
            // network finally has room is what gets chosen, so newer input always overtakes older video.
            let Ok(permit) = link_tx.reserve().await else {
                break;
            };
            while let Ok(m) = ctl_rx.try_recv() {
                push_ctl(&mut ctl, m, &c);
            }
            take_slot(&mut ctl, &c);

            let unit = if let Some(q) = ctl.pop_front() {
                if matches!(q.msg, Msg::InputAt { .. }) {
                    ewma(&c.input_queue_us, q.at.elapsed().as_micros() as u64);
                }
                let Ok(mut plain) = wire::encode(&q.msg) else {
                    continue;
                };
                plain.insert(0, (LANE_CTL << 2) | WHOLE);
                plain
            } else if let Some(v) = video.as_mut() {
                let (unit, done) = v.next_unit();
                if done {
                    c.video_send_us
                        .store(v.at.elapsed().as_micros() as u64, Ordering::Relaxed);
                    video = None;
                }
                unit
            } else if let Some(b) = bulk.as_mut() {
                let (unit, done) = b.next_unit();
                bucket.tokens -= unit.len() as f64;
                c.bulk_bytes_sent.fetch_add(unit.len() as u64, Ordering::Relaxed);
                if done {
                    bulk = None;
                }
                unit
            } else {
                continue;
            };
            let Ok(cipher) = sealer.seal(&unit) else { break };
            c.bytes_sent.fetch_add(cipher.len() as u64, Ordering::Relaxed);
            permit.send(cipher);
        }
        let _ = closed_w.send(true);
    });

    let c = counters.clone();
    tokio::spawn(async move {
        // Per-lane reassembly buffers for fragmented messages.
        let mut partial: [Vec<u8>; 3] = Default::default();
        // Only the first frame after an input sequence number changes shows that input's effect; later
        // frames carry the same number and must not be measured again.
        let mut last_measured_seq = 0u32;
        while let Some(frame) = link_rx.recv().await {
            c.bytes_received.fetch_add(frame.len() as u64, Ordering::Relaxed);
            let arrived = Instant::now();
            let Ok(plain) = opener.open(&frame) else {
                tracing::warn!(target: "security", "authentication failure on encrypted channel, closing session");
                break;
            };
            let Some((&hdr, body)) = plain.split_first() else {
                tracing::warn!(target: "security", "empty frame from peer, closing session");
                break;
            };
            let (lane, flag) = ((hdr >> 2) as usize, hdr & 3);
            if lane > 2 {
                tracing::warn!(target: "security", "unknown lane from peer, closing session");
                break;
            }
            let complete: Option<Vec<u8>> = match flag {
                WHOLE => Some(body.to_vec()),
                FIRST | MID | LAST => {
                    if flag == FIRST {
                        partial[lane].clear();
                    } else if partial[lane].is_empty() {
                        tracing::warn!(target: "security", "fragment without a start, closing session");
                        break;
                    }
                    if partial[lane].len() + body.len() > MAX_FRAME {
                        tracing::warn!(target: "security", "oversized message from peer, closing session");
                        break;
                    }
                    partial[lane].extend_from_slice(body);
                    (flag == LAST).then(|| std::mem::take(&mut partial[lane]))
                }
                _ => unreachable!("flag is two bits"),
            };
            let Some(bytes) = complete else { continue };
            let Ok(msg) = wire::decode::<Msg>(&bytes) else {
                tracing::warn!(target: "security", "malformed message from peer, closing session");
                break;
            };
            if let Err(why) = msg.validate() {
                tracing::warn!(target: "security", why, "invalid message from peer, closing session");
                break;
            }
            match msg {
                Msg::InputAt { seq, ev } => {
                    if let Some(route) = &input_route {
                        if route.send(InboundInput { seq, ev, arrived }).is_err() {
                            break;
                        }
                    }
                    continue;
                }
                Msg::InputAck { seq, .. } => {
                    if let Some(age) = c.clock.age_us(seq) {
                        ewma(&c.input_rtt_us, age);
                    }
                }
                Msg::Video(ref f) if f.input_seq != 0 && f.input_seq != last_measured_seq => {
                    last_measured_seq = f.input_seq;
                    if let Some(age) = c.clock.age_us(f.input_seq) {
                        ewma(&c.motion_to_frame_us, age);
                    }
                }
                _ => {}
            }
            if in_tx.send(msg).await.is_err() {
                break;
            }
        }
        let _ = closed_tx.send(true);
    });

    (
        SecureTx {
            ctl: ctl_tx,
            pending_move,
            wake,
            video: video_tx,
            bulk: bulk_tx,
            counters,
            kind,
        },
        SecureRx { rx: in_rx },
        closed_rx,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::memory_pair;
    use remotex_common::peer::{MouseButton, VideoFrame};

    fn channels() -> (SecureChannel, SecureChannel) {
        let (a, b) = ([7u8; 32], [9u8; 32]);
        (SecureChannel::new(&a, &b), SecureChannel::new(&b, &a))
    }

    fn video(len: usize) -> Msg {
        Msg::Video(VideoFrame {
            seq: 1,
            display: 0,
            codec: remotex_common::peer::Codec::H264,
            keyframe: true,
            width: 16,
            height: 16,
            captured_ms: 0,
            input_seq: 0,
            data: vec![3; len],
        })
    }

    #[tokio::test]
    async fn large_messages_are_fragmented_and_reassembled() {
        let (l1, l2) = memory_pair(LinkKind::Relay);
        let (c1, c2) = channels();
        let (tx, _rx1, _) = spawn(l1, c1, None);
        let (_tx2, mut rx2, _) = spawn(l2, c2, None);
        assert!(tx.try_send_video(video(200_000)));
        match rx2.recv().await.unwrap() {
            Msg::Video(f) => assert_eq!(f.data.len(), 200_000),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_click_overtakes_a_fragmented_frame_and_a_bulk_chunk() {
        // Deliberately slow consumer: only the receiving side's ordering matters.
        let (l1, l2) = memory_pair(LinkKind::Relay);
        let (c1, c2) = channels();
        let (route_tx, route_rx) = std_mpsc::channel();
        let (tx, _rx1, _) = spawn(l1, c1, None);
        let (_tx2, mut rx2, _) = spawn(l2, c2, Some(route_tx));
        assert!(tx.try_send_video(video(400_000)));
        tx.send_bulk(Msg::Chat {
            id: 1,
            text: "x".repeat(6000),
            ts_ms: 0,
        })
        .await;
        tx.send_input(InputEvent::MouseButton {
            button: MouseButton::Left,
            down: true,
        });
        // The click arrives on the input route long before the 400 KB frame is complete.
        let got = tokio::task::spawn_blocking(move || route_rx.recv_timeout(Duration::from_secs(2)))
            .await
            .unwrap()
            .expect("click delivered");
        assert!(matches!(got.ev, InputEvent::MouseButton { down: true, .. }));
        assert!(matches!(
            rx2.recv().await.unwrap(),
            Msg::Video(_) | Msg::Chat { .. }
        ));
    }

    #[tokio::test]
    async fn waiting_mouse_moves_collapse_but_clicks_survive_in_order() {
        let counters = NetCounters::default();
        let mut q = VecDeque::new();
        let mk = |ev| Queued {
            msg: Msg::InputAt { seq: 1, ev },
            at: Instant::now(),
        };
        for i in 0..40u16 {
            push_ctl(&mut q, mk(InputEvent::MouseMove { x: i, y: i }), &counters);
        }
        push_ctl(
            &mut q,
            mk(InputEvent::MouseButton {
                button: MouseButton::Left,
                down: true,
            }),
            &counters,
        );
        for i in 40..60u16 {
            push_ctl(&mut q, mk(InputEvent::MouseMove { x: i, y: i }), &counters);
        }
        push_ctl(
            &mut q,
            mk(InputEvent::MouseButton {
                button: MouseButton::Left,
                down: false,
            }),
            &counters,
        );
        let kinds: Vec<String> = q
            .iter()
            .map(|i| match &i.msg {
                Msg::InputAt {
                    ev: InputEvent::MouseMove { x, .. },
                    ..
                } => format!("move{x}"),
                Msg::InputAt {
                    ev: InputEvent::MouseButton { down, .. },
                    ..
                } => format!("btn{down}"),
                _ => "other".into(),
            })
            .collect();
        assert_eq!(kinds, ["move39", "btntrue", "move59", "btnfalse"]);
        assert_eq!(counters.moves_coalesced.load(Ordering::Relaxed), 58);
    }

    #[test]
    fn input_clock_measures_age_and_ignores_unknown_sequences() {
        let clock = InputClock::default();
        let seq = clock.stamp();
        std::thread::sleep(Duration::from_millis(5));
        assert!(clock.age_us(seq).unwrap() >= 4_000);
        assert!(clock.age_us(seq + 7).is_none());
        assert_ne!(clock.stamp(), 0);
    }
}
