//! Shared harness: an in-process server and engines that record every event they emit.
#![allow(dead_code)]

use remotex_server::config::Config;
use remotex_session::{
    testing::{test_env, MemoryStore, SharedClipboard, TestEnv},
    types::{EngineConfig, Event, ServerState},
    Engine,
};
use std::{
    net::TcpListener,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::Notify;

pub struct TestServer {
    pub url: String,
    /// SHA-256 of the server certificate when started with TLS.
    pub fingerprint: Option<String>,
    _dir: tempfile::TempDir,
    cfg: Arc<Config>,
    port: u16,
    stop: Arc<Notify>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl TestServer {
    fn launch(cfg: Arc<Config>, listener: TcpListener) -> (Arc<Notify>, tokio::task::JoinHandle<()>) {
        let stop = Arc::new(Notify::new());
        let s2 = stop.clone();
        let task = tokio::spawn(async move {
            let _ = remotex_server::serve(cfg, listener, async move { s2.notified().await }).await;
        });
        (stop, task)
    }

    pub async fn start() -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = tempfile::tempdir().unwrap();
        let cfg = Arc::new(Config::for_tests(dir.path().join("test.db")));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (stop, task) = Self::launch(cfg.clone(), listener);
        Self {
            url: format!("ws://127.0.0.1:{port}/ws"),
            fingerprint: None,
            _dir: dir,
            cfg,
            port,
            stop,
            task: Some(task),
        }
    }

    /// Serves TLS with a fresh self-signed certificate for 127.0.0.1.
    pub async fn start_tls() -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = tempfile::tempdir().unwrap();
        let fingerprint =
            remotex_server::tls::generate_self_signed(&["127.0.0.1".to_string()], &dir.path().join("certs"))
                .unwrap();
        let mut cfg = Config::for_tests(dir.path().join("test.db"));
        cfg.tls = Some((
            dir.path().join("certs/server.crt"),
            dir.path().join("certs/server.key"),
        ));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let cfg = Arc::new(cfg);
        let (stop, task) = Self::launch(cfg.clone(), listener);
        Self {
            url: format!("wss://127.0.0.1:{port}/ws"),
            fingerprint: Some(fingerprint),
            _dir: dir,
            cfg,
            port,
            stop,
            task: Some(task),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Takes the server down (existing connections are closed after the graceful-shutdown window).
    pub async fn stop_server(&mut self) {
        self.stop.notify_one();
        if let Some(t) = self.task.take() {
            let _ = t.await;
        }
    }

    /// Starts the same server again on the same port with the same database (a service restart).
    pub async fn start_again(&mut self) {
        let listener = loop {
            match TcpListener::bind(("127.0.0.1", self.port)) {
                Ok(l) => break l,
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        };
        let (stop, task) = Self::launch(self.cfg.clone(), listener);
        self.stop = stop;
        self.task = Some(task);
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.notify_one();
    }
}

/// `(keyframe, width, height, Annex-B bytes)`
pub type LoggedFrame = (bool, u32, u32, Vec<u8>);

#[derive(Default)]
pub struct Recorder {
    log: Mutex<Vec<Event>>,
    notify: Notify,
    pub video_frames: AtomicU64,
    pub last_video: Mutex<Option<(u32, u32, bool)>>,
    /// First frames received, kept for tests that inspect the raw bitstream.
    pub video_log: Mutex<Vec<LoggedFrame>>,
    pub last_stats: Mutex<Option<remotex_session::types::ViewerStats>>,
}

impl Recorder {
    pub fn mark(&self) -> usize {
        self.log.lock().unwrap().len()
    }

    /// Non-blocking check for an event at or after `from`.
    pub fn any_from(&self, from: usize, f: impl FnMut(&Event) -> bool) -> bool {
        self.log.lock().unwrap().iter().skip(from).any(f)
    }

    pub fn frames(&self) -> u64 {
        self.video_frames.load(Ordering::Relaxed)
    }

    /// Waits for the first event at or after `from` that satisfies `f`.
    pub async fn wait_from<T>(&self, from: usize, secs: u64, mut f: impl FnMut(&Event) -> Option<T>) -> T {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let log = self.log.lock().unwrap();
                if let Some(v) = log.iter().skip(from).find_map(&mut f) {
                    return v;
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                let log = self.log.lock().unwrap();
                let tail: Vec<String> = log
                    .iter()
                    .skip(from)
                    .rev()
                    .take(8)
                    .map(|e| format!("{e:?}"))
                    .collect();
                panic!("timed out waiting for event; recent events: {tail:#?}");
            }
        }
    }

    pub async fn wait<T>(&self, secs: u64, f: impl FnMut(&Event) -> Option<T>) -> T {
        self.wait_from(0, secs, f).await
    }
}

pub struct Node {
    pub engine: Engine,
    pub rec: Arc<Recorder>,
    pub env: TestEnv,
    pub id: String,
    pub password: String,
    pub downloads: tempfile::TempDir,
}

#[derive(Default)]
pub struct NodeOpts {
    pub direct: bool,
}

pub async fn node(server: &TestServer, name: &str, opts: NodeOpts) -> Node {
    node_with_env(server, name, opts, test_env()).await
}

pub async fn node_with_env(server: &TestServer, name: &str, opts: NodeOpts, env: TestEnv) -> Node {
    node_with_env_url(&server.url, name, opts, env).await
}

pub async fn node_with_env_url(url: &str, name: &str, opts: NodeOpts, env: TestEnv) -> Node {
    node_with_cfg(url, name, opts, env, NodeCfg::default()).await
}

/// Extra knobs for talking to a real (TLS, pinned) service instead of the in-process test server.
pub struct NodeCfg {
    pub pins: Vec<[u8; 32]>,
    pub allow_insecure: bool,
    pub store: Option<Arc<dyn remotex_session::signaling::CredentialStore>>,
}

impl Default for NodeCfg {
    fn default() -> Self {
        Self {
            pins: Vec::new(),
            allow_insecure: true,
            store: None,
        }
    }
}

pub async fn node_with_cfg(url: &str, name: &str, opts: NodeOpts, env: TestEnv, extra: NodeCfg) -> Node {
    let downloads = tempfile::tempdir().unwrap();
    let mut cfg = EngineConfig::new(url, name, PathBuf::from(downloads.path()));
    cfg.allow_insecure = extra.allow_insecure;
    cfg.cert_pins = extra.pins;
    cfg.direct_enabled = opts.direct;
    cfg.ask_before_receiving = false;
    cfg.approval_timeout = Duration::from_secs(5);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let store: Arc<dyn remotex_session::signaling::CredentialStore> =
        extra.store.unwrap_or_else(|| Arc::new(MemoryStore::default()));
    let engine = Engine::start(cfg, env.env.clone(), store, tx).await.unwrap();
    let rec = Arc::new(Recorder::default());
    let r2 = rec.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let Event::Video { frame, .. } = &ev {
                r2.video_frames.fetch_add(1, Ordering::Relaxed);
                *r2.last_video.lock().unwrap() = Some((frame.width, frame.height, frame.keyframe));
                let mut log = r2.video_log.lock().unwrap();
                if log.len() < 120 {
                    log.push((frame.keyframe, frame.width, frame.height, frame.data.clone()));
                }
                continue;
            }
            if let Event::ViewerStats { stats, .. } = &ev {
                *r2.last_stats.lock().unwrap() = Some(stats.clone());
                continue;
            }
            if matches!(ev, Event::Cursor { .. }) {
                continue;
            }
            r2.log.lock().unwrap().push(ev);
            r2.notify.notify_waiters();
        }
    });
    let id = rec
        .wait(10, |e| {
            if let Event::Identity { id } = e {
                Some(id.clone())
            } else {
                None
            }
        })
        .await;
    rec.wait(10, |e| {
        matches!(e, Event::Server(ServerState::Online)).then_some(())
    })
    .await;
    let password = engine.temp_password();
    Node {
        engine,
        rec,
        env,
        id,
        password,
        downloads,
    }
}

pub fn clipboard(n: &Node) -> &Arc<Mutex<SharedClipboard>> {
    &n.env.clipboard
}

/// A TCP proxy that emulates a network path: fixed one-way delay, a bandwidth cap and optional
/// random stalls, applied independently to each direction of every connection.
pub struct NetEm {
    pub port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl NetEm {
    /// `delay` is one way; `kbps == 0` means unlimited.
    pub async fn start(target_port: u16, delay: Duration, kbps: u64) -> NetEm {
        Self::start_lossy(target_port, delay, kbps, 0.0).await
    }

    /// `loss` is the chance (0..1) that a chunk is "lost" and recovered by TCP after a retransmission
    /// timeout: everything behind it waits, exactly like head-of-line blocking on a lossy link.
    pub async fn start_lossy(target_port: u16, delay: Duration, kbps: u64, loss: f64) -> NetEm {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            loop {
                let Ok((client, _)) = listener.accept().await else {
                    return;
                };
                let Ok(server) = tokio::net::TcpStream::connect(("127.0.0.1", target_port)).await else {
                    continue;
                };
                let _ = client.set_nodelay(true);
                let _ = server.set_nodelay(true);
                let (cr, cw) = client.into_split();
                let (sr, sw) = server.into_split();
                tokio::spawn(pipe(cr, sw, delay, kbps, loss));
                tokio::spawn(pipe(sr, cw, delay, kbps, loss));
            }
        });
        NetEm { port, task }
    }
}

impl Drop for NetEm {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn pipe(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    delay: Duration,
    kbps: u64,
    loss: f64,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(tokio::time::Instant, Vec<u8>)>();
    let writer = tokio::spawn(async move {
        while let Some((at, buf)) = rx.recv().await {
            if at > tokio::time::Instant::now() {
                tokio::time::sleep_until(at).await;
            }
            if to.write_all(&buf).await.is_err() {
                break;
            }
        }
        let _ = to.shutdown().await;
    });
    let mut buf = vec![0u8; 8192];
    // Serialisation on the emulated bottleneck: the link is busy until `link_free`.
    let mut link_free = tokio::time::Instant::now();
    while let Ok(n) = from.read(&mut buf).await {
        if n == 0 {
            break;
        }
        let now = tokio::time::Instant::now();
        let mut ready = now.max(link_free);
        if loss > 0.0 && rand_unit() < loss {
            // Retransmission timeout: the lost chunk and everything queued behind it is delayed.
            ready += Duration::from_millis(200).max(delay * 2);
            link_free = ready;
        }
        if kbps > 0 {
            ready += Duration::from_secs_f64(n as f64 * 8.0 / (kbps as f64 * 1000.0));
            link_free = ready;
            // Reading pauses while the bottleneck is busy, so the sender's socket buffer fills as on a real link.
            if ready > now + Duration::from_millis(1) {
                tokio::time::sleep_until(ready).await;
            }
        }
        if tx.send((ready + delay, buf[..n].to_vec())).is_err() {
            break;
        }
    }
    drop(tx);
    let _ = writer.await;
}

fn rand_unit() -> f64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static STATE: AtomicU64 = AtomicU64::new(0x9E3779B97F4A7C15);
    let mut x = STATE.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    (x >> 11) as f64 / (1u64 << 53) as f64
}
