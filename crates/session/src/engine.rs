//! The engine ties signaling, sessions and settings together and exposes the API the UI drives.

use crate::{
    env::PlatformEnv,
    host::{HostCmd, HostHandle},
    link::{Link, LinkKind},
    signaling::{self, CredentialStore, SigEvent, SignalConfig, SignalInputs, Signaling},
    types::{EngineConfig, Event, EventTx, ServerState},
    viewer::{ViewerCmd, ViewerHandle},
};
use remotex_common::{
    peer::{AuthKind, Permissions},
    signaling::{ClientToServer, ConnectFailure, ServerToClient},
    DeviceId, SessionId,
};
use remotex_security::{limiter::AttemptLimiter, password};
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{atomic::AtomicU64, Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, watch},
    time::timeout,
};

pub(crate) struct ReadyInfo {
    pub session: SessionId,
    pub candidates: Vec<String>,
}

pub(crate) type ConnectResult = Result<ReadyInfo, ConnectFailure>;

pub(crate) struct HostState {
    pub temp: String,
    pub temp_created: Instant,
    pub busy: bool,
    pub active: Option<HostHandle>,
    pub pending: HashMap<u64, oneshot::Sender<Option<Permissions>>>,
    pub per_viewer_failures: AttemptLimiter<String>,
    pub global_failures: AttemptLimiter<()>,
    pub resume_grants: HashMap<String, (Permissions, Instant)>,
    pub unattended_enabled: bool,
    pub unattended_any_device: bool,
    /// Whether unattended (permanent password) sessions may be elevated. Off unless the owner turns it on.
    pub unattended_elevated: bool,
    pub authorized: std::collections::HashSet<String>,
    pub permanent_password: Option<String>,
}

pub(crate) struct Inner {
    /// Handle of the runtime the engine was started on, so sync callers (UI command threads) can spawn work.
    pub rt: tokio::runtime::Handle,
    pub cfg: Mutex<Arc<EngineConfig>>,
    pub env: PlatformEnv,
    pub events: EventTx,
    pub sig: Signaling,
    pub routes: Mutex<HashMap<SessionId, mpsc::Sender<Vec<u8>>>>,
    pub pending_connect: Mutex<HashMap<String, oneshot::Sender<ConnectResult>>>,
    pub host: Mutex<HostState>,
    pub viewers: Mutex<HashMap<u64, ViewerHandle>>,
    pub my_id: Mutex<Option<String>>,
    pub server_online: Mutex<bool>,
    pub next_session_no: AtomicU64,
    pub next_request: AtomicU64,
    pub incoming_enabled: watch::Sender<bool>,
    pub pending_direct: Mutex<HashMap<SessionId, oneshot::Sender<TcpStream>>>,
}

#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Inner {
    pub(crate) fn cfg(&self) -> Arc<EngineConfig> {
        lock(&self.cfg).clone()
    }

    pub(crate) fn host(&self) -> MutexGuard<'_, HostState> {
        lock(&self.host)
    }

    pub(crate) fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    /// A link whose frames travel through the server relay for `session`.
    pub(crate) fn relay_link(&self, session: SessionId) -> Link {
        let (in_tx, in_rx) = mpsc::channel(256);
        lock(&self.routes).insert(session, in_tx);
        let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(crate::link::OUT_DEPTH);
        let sig = self.sig.clone();
        tokio::spawn(async move {
            while let Some(data) = out_rx.recv().await {
                if !sig.relay(session, data).await {
                    break;
                }
            }
        });
        Link::new(LinkKind::Relay, out_tx, in_rx, "relay".into())
    }

    pub(crate) fn drop_route(&self, session: &SessionId) {
        lock(&self.routes).remove(session);
    }

    fn rotate_password(&self) -> String {
        let pw = password::generate_temp_password();
        {
            let mut h = self.host();
            h.temp = pw.clone();
            h.temp_created = Instant::now();
        }
        let ttl = self.cfg().temp_password_ttl.map(|d| d.as_secs());
        self.emit(Event::TempPassword {
            password: pw.clone(),
            expires_in_secs: ttl,
        });
        pw
    }
}

impl Engine {
    /// Starts networking. Returns immediately; state changes arrive as events.
    pub async fn start(
        cfg: EngineConfig,
        env: PlatformEnv,
        store: Arc<dyn CredentialStore>,
        events: EventTx,
    ) -> std::io::Result<Engine> {
        let (incoming_tx, incoming_rx) = watch::channel(cfg.incoming_enabled);

        let (listener, direct_port) = if cfg.direct_enabled {
            let l = TcpListener::bind(("0.0.0.0", 0)).await?;
            let port = l.local_addr()?.port();
            (Some(l), Some(port))
        } else {
            (None, None)
        };
        let candidates: Vec<String> = match direct_port {
            Some(port) => (env.local_addresses)()
                .into_iter()
                .map(|ip| std::net::SocketAddr::new(ip, port).to_string())
                .collect(),
            None => Vec::new(),
        };
        let (cand_tx, cand_rx) = watch::channel(candidates);
        std::mem::forget(cand_tx); // candidates are fixed for the lifetime of the process

        let (sig_tx, mut sig_rx) = mpsc::unbounded_channel();
        let sig = signaling::spawn(
            SignalConfig {
                url: cfg.server_url.clone(),
                pins: cfg.cert_pins.clone(),
                allow_insecure: cfg.allow_insecure,
                client_version: cfg.client_version.clone(),
                device_name: cfg.device_name.clone(),
            },
            store,
            SignalInputs {
                candidates: cand_rx,
                incoming_enabled: incoming_rx,
            },
            sig_tx,
        );

        let temp = password::generate_temp_password();
        let inner = Arc::new(Inner {
            rt: tokio::runtime::Handle::current(),
            cfg: Mutex::new(Arc::new(cfg)),
            env,
            events,
            sig,
            routes: Mutex::new(HashMap::new()),
            pending_connect: Mutex::new(HashMap::new()),
            host: Mutex::new(HostState {
                temp,
                temp_created: Instant::now(),
                busy: false,
                active: None,
                pending: HashMap::new(),
                per_viewer_failures: AttemptLimiter::new(
                    5,
                    Duration::from_secs(600),
                    Duration::from_secs(1800),
                ),
                global_failures: AttemptLimiter::new(10, Duration::from_secs(600), Duration::from_secs(600)),
                resume_grants: HashMap::new(),
                unattended_enabled: false,
                unattended_any_device: false,
                unattended_elevated: false,
                authorized: Default::default(),
                permanent_password: None,
            }),
            viewers: Mutex::new(HashMap::new()),
            my_id: Mutex::new(None),
            server_online: Mutex::new(false),
            next_session_no: AtomicU64::new(1),
            next_request: AtomicU64::new(1),
            incoming_enabled: incoming_tx,
            pending_direct: Mutex::new(HashMap::new()),
        });

        let engine = Engine { inner: inner.clone() };
        inner.emit(Event::Server(ServerState::Starting));
        {
            let (pw, ttl) = (
                inner.host().temp.clone(),
                inner.cfg().temp_password_ttl.map(|d| d.as_secs()),
            );
            inner.emit(Event::TempPassword {
                password: pw,
                expires_in_secs: ttl,
            });
        }

        let inner2 = inner.clone();
        tokio::spawn(async move {
            while let Some(ev) = sig_rx.recv().await {
                dispatch(&inner2, ev).await;
            }
        });
        if let Some(l) = listener {
            tokio::spawn(accept_direct(inner.clone(), l));
        }
        tokio::spawn(password_expiry(inner.clone()));
        Ok(engine)
    }

    // ---- identity & presence -------------------------------------------------------------------

    pub fn my_id(&self) -> Option<DeviceId> {
        lock(&self.inner.my_id).as_deref().and_then(DeviceId::parse)
    }

    pub async fn query_presence(&self, ids: Vec<String>) {
        self.inner.sig.send(ClientToServer::QueryPresence { ids }).await;
    }

    pub fn server_online(&self) -> bool {
        *lock(&self.inner.server_online)
    }

    // ---- host settings -------------------------------------------------------------------------

    pub fn temp_password(&self) -> String {
        self.inner.host().temp.clone()
    }

    pub fn regenerate_password(&self) -> String {
        self.inner.rotate_password()
    }

    pub fn set_incoming_enabled(&self, enabled: bool) {
        let _ = self.inner.incoming_enabled.send(enabled);
    }

    /// Enables or disables unattended access. The password is kept in memory only; persistence
    /// (encrypted with DPAPI) is the caller's responsibility.
    pub fn set_unattended(
        &self,
        enabled: bool,
        permanent_password: Option<String>,
        authorized: Vec<String>,
        any_device: bool,
    ) {
        let mut h = self.inner.host();
        h.unattended_enabled = enabled && permanent_password.as_ref().is_some_and(|p| p.len() >= 8);
        h.permanent_password = if h.unattended_enabled {
            permanent_password
        } else {
            None
        };
        h.unattended_any_device = any_device;
        h.authorized = authorized.into_iter().collect();
    }

    /// Allows (or forbids) elevated control in unattended sessions. Separate from unattended access itself.
    pub fn set_unattended_elevated(&self, on: bool) {
        self.inner.host().unattended_elevated = on;
    }

    /// Whether the elevated-control service is installed and answering.
    pub fn elevation_available(&self) -> bool {
        self.inner.env.elevation.available()
    }

    /// Ids allowed to connect unattended (a subset of trusted devices).
    pub fn revoke_authorization(&self, id: &str) {
        self.inner.host().authorized.remove(id);
    }

    pub fn update_config(&self, cfg: EngineConfig) {
        *lock(&self.inner.cfg) = Arc::new(cfg);
    }

    pub fn respond_incoming(&self, request_id: u64, grant: Option<Permissions>) {
        if let Some(tx) = self.inner.host().pending.remove(&request_id) {
            let _ = tx.send(grant);
        }
    }

    pub fn host_command(&self, cmd: HostCmd) {
        if let Some(h) = self.inner.host().active.as_ref() {
            let _ = h.cmd.send(cmd);
        }
    }

    /// Test hook: abruptly cuts every session link, as a network failure would.
    #[cfg(any(test, feature = "testing"))]
    pub fn sever_links(&self) {
        lock(&self.inner.routes).clear();
    }

    pub fn host_active(&self) -> bool {
        self.inner.host().active.is_some()
    }

    // ---- viewer --------------------------------------------------------------------------------

    pub fn viewer_command(&self, session: u64, cmd: ViewerCmd) {
        if let Some(v) = lock(&self.inner.viewers).get(&session) {
            match cmd {
                // Input takes the direct path; while reconnecting there is no live channel and it is dropped.
                ViewerCmd::Input(ev) => {
                    if let Some(tx) = lock(&v.input).as_ref() {
                        tx.send_input(ev);
                    }
                }
                other => {
                    let _ = v.cmd.send(other);
                }
            }
        }
    }

    pub fn available_encoders(&self) -> Vec<(remotex_common::peer::Codec, String, bool)> {
        self.inner.env.encoders.available()
    }

    pub fn connect(
        &self,
        target: &str,
        password: &str,
        auth: AuthKind,
        requested: Permissions,
    ) -> Result<u64, ConnectFailure> {
        crate::engine_viewer::start(&self.inner, target, password, auth, requested)
    }
}

async fn dispatch(inner: &Arc<Inner>, ev: SigEvent) {
    match ev {
        SigEvent::State(s) => {
            *lock(&inner.server_online) = matches!(s, ServerState::Online);
            inner.emit(Event::Server(s));
        }
        SigEvent::Identity(id) => {
            *lock(&inner.my_id) = Some(id.clone());
            inner.emit(Event::Identity { id });
        }
        SigEvent::Msg(msg) => match msg {
            ServerToClient::Incoming {
                session,
                from_id,
                from_name,
                same_network,
            } => {
                let inner = inner.clone();
                tokio::spawn(async move {
                    crate::engine_host::handle_incoming(inner, session, from_id, from_name, same_network)
                        .await
                });
            }
            ServerToClient::SessionReady {
                session, candidates, ..
            } => {
                // Exactly one connect per target is pending at a time; resolve it.
                let pending: Vec<_> = lock(&inner.pending_connect).drain().collect();
                if let Some((_, tx)) = pending.into_iter().next() {
                    let _ = tx.send(Ok(ReadyInfo { session, candidates }));
                }
            }
            ServerToClient::ConnectFailed { target_id, reason } => {
                if let Some(tx) = lock(&inner.pending_connect).remove(&target_id) {
                    let _ = tx.send(Err(reason));
                } else if let Some((_, tx)) = lock(&inner.pending_connect).drain().next() {
                    let _ = tx.send(Err(reason));
                }
            }
            ServerToClient::Relay { session, data } => {
                let tx = lock(&inner.routes).get(&session).cloned();
                if let Some(tx) = tx {
                    if timeout(Duration::from_secs(5), tx.send(data))
                        .await
                        .map(|r| r.is_err())
                        .unwrap_or(true)
                    {
                        inner.drop_route(&session);
                    }
                }
            }
            ServerToClient::SessionEnded { session } => inner.drop_route(&session),
            ServerToClient::Presence(list) => inner.emit(Event::Presence { statuses: list }),
            ServerToClient::Error { message } => inner.emit(Event::Security { message }),
            ServerToClient::Registered { .. }
            | ServerToClient::AuthOk { .. }
            | ServerToClient::AuthFailed { .. }
            | ServerToClient::Pong => {}
        },
    }
}

/// Accepts direct (LAN) connections, but only for sessions the server has announced.
async fn accept_direct(inner: Arc<Inner>, listener: TcpListener) {
    let gate = Arc::new(tokio::sync::Semaphore::new(16));
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            continue;
        };
        let Ok(permit) = gate.clone().try_acquire_owned() else {
            continue;
        };
        let inner = inner.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let mut preamble = [0u8; 16];
            if timeout(Duration::from_secs(3), stream.read_exact(&mut preamble))
                .await
                .map(|r| r.is_err())
                .unwrap_or(true)
            {
                return;
            }
            let tx = lock(&inner.pending_direct).remove(&SessionId(preamble));
            if let Some(tx) = tx {
                if stream.write_all(&[1]).await.is_ok() {
                    let _ = tx.send(stream);
                }
            }
        });
    }
}

async fn password_expiry(inner: Arc<Inner>) {
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tick.tick().await;
        let Some(ttl) = inner.cfg().temp_password_ttl else {
            continue;
        };
        let expired = { inner.host().temp_created.elapsed() >= ttl };
        if expired && !inner.host().busy {
            inner.rotate_password();
        }
    }
}

/// Non-loopback, non-link-local IPv4 addresses are the useful LAN candidates.
pub fn is_lan_candidate(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified(),
        IpAddr::V6(_) => false,
    }
}

impl Inner {
    pub(crate) fn rotate_after_session(&self) {
        self.rotate_password();
    }
}
