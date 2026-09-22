//! In-memory presence and session state. All rules about who may talk to whom live here.

use crate::config::Config;
use remotex_common::{
    signaling::{ConnectFailure, ServerToClient},
    wire, DeviceId, SessionId,
};
use remotex_security::{ids::generate_session_id, limiter::AttemptLimiter};
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, Notify};

pub type Outbox = mpsc::Sender<Vec<u8>>;

pub fn encode(msg: &ServerToClient) -> Vec<u8> {
    wire::encode(msg).expect("server messages always fit")
}

pub struct Conn {
    pub conn_id: u64,
    pub tx: Outbox,
    pub kick: Arc<Notify>,
    pub name: String,
    pub ip: IpAddr,
    pub candidates: Vec<String>,
    pub incoming_enabled: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Pending,
    Relay,
    Direct,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Pending => "pending",
            Mode::Relay => "relay",
            Mode::Direct => "direct",
        }
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

struct Session {
    viewer: String,
    host: String,
    viewer_ip: IpAddr,
    created: Instant,
    last_activity: Instant,
    mode: Mode,
    bytes: u64,
    bucket: Bucket,
}

struct Limiters {
    auth_fail_ip: AttemptLimiter<IpAddr>,
    register_ip: AttemptLimiter<IpAddr>,
    connect_src: AttemptLimiter<String>,
    enumerate_ip: AttemptLimiter<IpAddr>,
    pw_fail_viewer: AttemptLimiter<String>,
    pw_fail_ip: AttemptLimiter<IpAddr>,
    admin_fail_ip: AttemptLimiter<IpAddr>,
}

struct Inner {
    devices: HashMap<String, Conn>,
    sessions: HashMap<SessionId, Session>,
    in_session: HashMap<String, SessionId>,
    lim: Limiters,
}

#[derive(Default)]
pub struct Metrics {
    pub relay_bytes: AtomicU64,
    pub relay_bps: AtomicU64,
    pub sessions_total: AtomicU64,
    pub connects_rejected: AtomicU64,
    pub auth_failures: AtomicU64,
    pub errors: AtomicU64,
}

#[derive(Default, Clone)]
pub struct SysSnapshot {
    pub cpu_percent: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub process_mem: u64,
}

pub struct Hub {
    pub cfg: Arc<Config>,
    pub metrics: Metrics,
    pub sys: Mutex<SysSnapshot>,
    pub started: Instant,
    /// Flips to true when the service is stopping so every connection can close cleanly.
    pub shutdown: tokio::sync::watch::Sender<bool>,
    next_conn: AtomicU64,
    inner: Mutex<Inner>,
}

pub struct Ended {
    pub session: SessionId,
    pub viewer: String,
    pub host: String,
    pub mode: Mode,
    pub bytes: u64,
    notify: Vec<Outbox>,
}

impl Ended {
    /// Tells the surviving peers the session is over.
    pub fn notify(&self) {
        let msg = encode(&ServerToClient::SessionEnded {
            session: self.session,
        });
        for tx in &self.notify {
            let _ = tx.try_send(msg.clone());
        }
    }
}

pub struct Started {
    pub session: SessionId,
    pub host_tx: Outbox,
    pub host_name: String,
    pub viewer_name: String,
    pub same_network: bool,
    pub host_candidates: Vec<String>,
    pub replaced: Option<Ended>,
}

pub struct RelayRoute {
    pub tx: Outbox,
    pub delay: Duration,
}

pub struct Stats {
    pub online: usize,
    pub sessions: usize,
    pub direct: usize,
    pub relay: usize,
    pub pending: usize,
}

const MINUTE: Duration = Duration::from_secs(60);

impl Hub {
    pub fn new(cfg: Arc<Config>) -> Arc<Self> {
        let lim = Limiters {
            auth_fail_ip: AttemptLimiter::new(10, 5 * MINUTE, 15 * MINUTE),
            register_ip: AttemptLimiter::new(30, 60 * MINUTE, 15 * MINUTE),
            connect_src: AttemptLimiter::new(30, MINUTE, 5 * MINUTE),
            enumerate_ip: AttemptLimiter::new(15, 10 * MINUTE, 30 * MINUTE),
            pw_fail_viewer: AttemptLimiter::new(5, 10 * MINUTE, 30 * MINUTE),
            pw_fail_ip: AttemptLimiter::new(10, 10 * MINUTE, 30 * MINUTE),
            admin_fail_ip: AttemptLimiter::new(5, 5 * MINUTE, 15 * MINUTE),
        };
        Arc::new(Self {
            cfg,
            metrics: Metrics::default(),
            sys: Mutex::new(SysSnapshot::default()),
            started: Instant::now(),
            shutdown: tokio::sync::watch::channel(false).0,
            next_conn: AtomicU64::new(1),
            inner: Mutex::new(Inner {
                devices: HashMap::new(),
                sessions: HashMap::new(),
                in_session: HashMap::new(),
                lim,
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn next_conn_id(&self) -> u64 {
        self.next_conn.fetch_add(1, Ordering::Relaxed)
    }

    // ---- rate limiting -------------------------------------------------------------------------

    pub fn ip_banned_for_auth(&self, ip: IpAddr) -> bool {
        self.lock().lim.auth_fail_ip.locked(&ip, Instant::now()).is_some()
    }

    /// Returns true if this failure caused a ban.
    pub fn record_auth_failure(&self, ip: IpAddr) -> bool {
        self.metrics.auth_failures.fetch_add(1, Ordering::Relaxed);
        self.lock().lim.auth_fail_ip.record_failure(&ip, Instant::now())
    }

    /// Counts a registration; false means the IP exceeded its hourly allowance.
    pub fn allow_register(&self, ip: IpAddr) -> bool {
        let mut g = self.lock();
        let now = Instant::now();
        if g.lim.register_ip.locked(&ip, now).is_some() {
            return false;
        }
        g.lim.register_ip.record_failure(&ip, now);
        true
    }

    pub fn admin_locked(&self, ip: IpAddr) -> bool {
        self.lock()
            .lim
            .admin_fail_ip
            .locked(&ip, Instant::now())
            .is_some()
    }

    pub fn admin_failure(&self, ip: IpAddr) {
        self.lock().lim.admin_fail_ip.record_failure(&ip, Instant::now());
    }

    // ---- presence ------------------------------------------------------------------------------

    /// Registers the connection; a previous connection for the same device is kicked.
    pub fn attach(&self, id: &str, conn: Conn) {
        let mut g = self.lock();
        if let Some(old) = g.devices.insert(id.to_string(), conn) {
            old.kick.notify_one();
        }
    }

    pub fn update_incoming(&self, id: &str, conn_id: u64, enabled: bool) {
        if let Some(c) = self.lock().devices.get_mut(id) {
            if c.conn_id == conn_id {
                c.incoming_enabled = enabled;
            }
        }
    }

    /// Removes the device (if this connection still owns the slot) and ends its sessions.
    pub fn detach(&self, id: &str, conn_id: u64) -> Vec<Ended> {
        let mut g = self.lock();
        if g.devices.get(id).map(|c| c.conn_id) != Some(conn_id) {
            return Vec::new();
        }
        g.devices.remove(id);
        let mut ended = Vec::new();
        if let Some(sid) = g.in_session.get(id).copied() {
            if let Some(e) = Self::end_locked(&mut g, sid) {
                ended.push(e);
            }
        }
        ended
    }

    pub fn presence(&self, ids: &[String]) -> Vec<(String, bool)> {
        let g = self.lock();
        ids.iter()
            .map(|i| (i.clone(), g.devices.get(i).is_some_and(|c| c.incoming_enabled)))
            .collect()
    }

    // ---- sessions ------------------------------------------------------------------------------

    pub fn connect(&self, viewer: &str, viewer_conn: u64, target: &str) -> Result<Started, ConnectFailure> {
        let now = Instant::now();
        let mut g = self.lock();
        let viewer_ip = g
            .devices
            .get(viewer)
            .map(|c| c.ip)
            .ok_or(ConnectFailure::Offline)?;
        let viewer_name = g.devices.get(viewer).map(|c| c.name.clone()).unwrap_or_default();
        let reject = |m: &Metrics, f: ConnectFailure| {
            m.connects_rejected.fetch_add(1, Ordering::Relaxed);
            Err(f)
        };

        if g.lim.connect_src.locked(&viewer.to_string(), now).is_some() {
            return reject(&self.metrics, ConnectFailure::RateLimited);
        }
        g.lim.connect_src.record_failure(&viewer.to_string(), now);
        if g.lim.pw_fail_viewer.locked(&viewer.to_string(), now).is_some()
            || g.lim.pw_fail_ip.locked(&viewer_ip, now).is_some()
            || g.lim.enumerate_ip.locked(&viewer_ip, now).is_some()
        {
            return reject(&self.metrics, ConnectFailure::Banned);
        }
        let Some(target_id) = DeviceId::parse(target) else {
            g.lim.enumerate_ip.record_failure(&viewer_ip, now);
            return reject(&self.metrics, ConnectFailure::InvalidId);
        };
        if target_id.as_str() == viewer {
            return reject(&self.metrics, ConnectFailure::InvalidId);
        }
        let Some(host) = g.devices.get(target_id.as_str()) else {
            g.lim.enumerate_ip.record_failure(&viewer_ip, now);
            return reject(&self.metrics, ConnectFailure::Offline);
        };
        if !host.incoming_enabled {
            return reject(&self.metrics, ConnectFailure::NotAccepting);
        }
        let (host_tx, host_name, host_ip, host_candidates) = (
            host.tx.clone(),
            host.name.clone(),
            host.ip,
            host.candidates.clone(),
        );

        // A viewer reconnecting replaces its own previous session.
        let mut replaced = None;
        if let Some(old) = g.in_session.get(viewer).copied() {
            if g.sessions.get(&old).is_some_and(|s| s.viewer == viewer) {
                replaced = Self::end_locked(&mut g, old);
            }
        }
        if g.in_session.contains_key(target_id.as_str()) || g.in_session.contains_key(viewer) {
            return reject(&self.metrics, ConnectFailure::Busy);
        }
        if g.sessions.len() >= self.cfg.max_sessions {
            return reject(&self.metrics, ConnectFailure::Busy);
        }

        let session = generate_session_id();
        let same_network = host_ip == viewer_ip;
        let rate = self.cfg.max_relay_mbps_per_session as f64 * 125_000.0;
        g.sessions.insert(
            session,
            Session {
                viewer: viewer.to_string(),
                host: target_id.as_str().to_string(),
                viewer_ip,
                created: now,
                last_activity: now,
                mode: Mode::Pending,
                bytes: 0,
                bucket: Bucket {
                    tokens: rate * 0.25,
                    last: now,
                },
            },
        );
        g.in_session.insert(viewer.to_string(), session);
        g.in_session.insert(target_id.as_str().to_string(), session);
        let _ = viewer_conn;
        self.metrics.sessions_total.fetch_add(1, Ordering::Relaxed);
        Ok(Started {
            session,
            host_tx,
            host_name,
            viewer_name,
            same_network,
            // LAN addresses are only disclosed to a viewer behind the same public address.
            host_candidates: if same_network { host_candidates } else { Vec::new() },
            replaced,
        })
    }

    /// Peer of `from` in `session`, or None if `from` is not a participant.
    pub fn relay_route(&self, session: &SessionId, from: &str, len: usize) -> Option<RelayRoute> {
        let now = Instant::now();
        let mut g = self.lock();
        let s = g.sessions.get_mut(session)?;
        let peer = if s.viewer == from {
            s.host.clone()
        } else if s.host == from {
            s.viewer.clone()
        } else {
            return None;
        };
        if s.mode == Mode::Pending {
            s.mode = Mode::Relay;
        }
        if s.mode != Mode::Relay {
            return None;
        }
        s.last_activity = now;
        s.bytes += len as u64;
        self.metrics.relay_bytes.fetch_add(len as u64, Ordering::Relaxed);

        let rate = self.cfg.max_relay_mbps_per_session as f64 * 125_000.0;
        let b = &mut s.bucket;
        b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * rate).min(rate * 0.25);
        b.last = now;
        b.tokens -= len as f64;
        let delay = if b.tokens < 0.0 {
            Duration::from_secs_f64(-b.tokens / rate)
        } else {
            Duration::ZERO
        };

        let tx = g.devices.get(&peer)?.tx.clone();
        Some(RelayRoute { tx, delay })
    }

    pub fn set_mode(&self, session: &SessionId, from: &str, direct: bool) {
        let mut g = self.lock();
        if let Some(s) = g.sessions.get_mut(session) {
            if s.viewer == from && s.mode == Mode::Pending {
                s.mode = if direct { Mode::Direct } else { Mode::Relay };
                s.last_activity = Instant::now();
            }
        }
    }

    /// Ends a session on behalf of a participant. Returns the summary if it existed.
    pub fn end_by(&self, session: &SessionId, by: &str) -> Option<Ended> {
        let mut g = self.lock();
        let s = g.sessions.get(session)?;
        if s.viewer != by && s.host != by {
            return None;
        }
        Self::end_locked(&mut g, *session)
    }

    /// Host reported a failed password. Returns true if that triggered a temporary ban.
    pub fn report_password_failure(&self, session: &SessionId, host: &str) -> Option<(String, bool)> {
        let now = Instant::now();
        let mut g = self.lock();
        let s = g.sessions.get(session)?;
        if s.host != host {
            return None;
        }
        let (viewer, ip) = (s.viewer.clone(), s.viewer_ip);
        let a = g.lim.pw_fail_viewer.record_failure(&viewer, now);
        let b = g.lim.pw_fail_ip.record_failure(&ip, now);
        Some((viewer, a || b))
    }

    fn end_locked(g: &mut Inner, sid: SessionId) -> Option<Ended> {
        let s = g.sessions.remove(&sid)?;
        g.in_session.remove(&s.viewer);
        g.in_session.remove(&s.host);
        let notify = [&s.viewer, &s.host]
            .iter()
            .filter_map(|id| g.devices.get(*id).map(|c| c.tx.clone()))
            .collect();
        Some(Ended {
            session: sid,
            viewer: s.viewer,
            host: s.host,
            mode: s.mode,
            bytes: s.bytes,
            notify,
        })
    }

    /// Periodic cleanup of abandoned sessions and limiter state.
    pub fn sweep(&self) -> Vec<Ended> {
        let now = Instant::now();
        let mut g = self.lock();
        let stale: Vec<SessionId> = g
            .sessions
            .iter()
            .filter(|(_, s)| match s.mode {
                Mode::Pending => now.duration_since(s.created) > Duration::from_secs(60),
                Mode::Relay => now.duration_since(s.last_activity) > Duration::from_secs(120),
                Mode::Direct => false,
            })
            .map(|(id, _)| *id)
            .collect();
        let ended = stale
            .into_iter()
            .filter_map(|id| Self::end_locked(&mut g, id))
            .collect();
        let l = &mut g.lim;
        l.auth_fail_ip.sweep(now);
        l.register_ip.sweep(now);
        l.connect_src.sweep(now);
        l.enumerate_ip.sweep(now);
        l.pw_fail_viewer.sweep(now);
        l.pw_fail_ip.sweep(now);
        l.admin_fail_ip.sweep(now);
        ended
    }

    pub fn stats(&self) -> Stats {
        let g = self.lock();
        let count = |m: Mode| g.sessions.values().filter(|s| s.mode == m).count();
        Stats {
            online: g.devices.len(),
            sessions: g.sessions.len(),
            direct: count(Mode::Direct),
            relay: count(Mode::Relay),
            pending: count(Mode::Pending),
        }
    }
}
