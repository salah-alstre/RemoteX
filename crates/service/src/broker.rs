//! The service's decision logic: who may ask for elevated control, what a grant allows, and when it ends.
//!
//! Rules enforced here (not in the app, not in the agent):
//! * Only the installed RemoteX executable, running in the active console session, may open a grant.
//! * At most one grant exists; it is bound to that process and to a random token.
//! * A grant lapses by itself without heartbeats and ends the moment its owner disconnects.
//! * Only [`UiToService`] operations exist; nothing a caller sends can name a program, a path or a command.
//! * Every request is validated (sizes, ranges) before it is relayed to the agent.

use crate::launch::{spawn_agent, Child, LaunchMode};
use remotex_common::peer::{DisplayInfo, InputEvent};
use remotex_elevate::{
    pipe::{self, PipeSecurity},
    read_msg, write_msg, AgentToService, ConnRole, DenyReason, Picture, ServiceToAgent, GRANT_LAPSE_SECS,
};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LaunchKind {
    /// Production: the agent is a SYSTEM process inside the console session.
    System,
    /// Development/tests without administrator rights: the agent runs as the caller's account.
    SameUser,
}

pub struct BrokerConfig {
    /// The executable started as the agent (this program).
    pub exe: PathBuf,
    /// Executables allowed to open a grant.
    pub allowed_ui: Vec<PathBuf>,
    pub launch: LaunchKind,
    /// Require the caller to live in the active console session (always true in production).
    pub enforce_console_session: bool,
    /// Refuse grants unless the installation directory is one only administrators can write to.
    pub require_protected_install: bool,
}

struct Agent {
    input: Mutex<File>,
    video: Mutex<File>,
    child: Mutex<Child>,
}

impl Agent {
    fn shutdown(&self) {
        for pipe in [&self.input, &self.video] {
            if let Ok(mut f) = pipe.lock() {
                let _ = write_msg(&mut *f, &ServiceToAgent::Shutdown);
            }
        }
        if let Ok(mut c) = self.child.lock() {
            c.kill();
        }
    }
}

struct Grant {
    token: [u8; 16],
    owner_pid: u32,
    label: String,
    last_beat: Mutex<Instant>,
    agent: Agent,
}

pub struct Broker {
    cfg: BrokerConfig,
    grant: Mutex<Option<Arc<Grant>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

impl Broker {
    pub fn new(cfg: BrokerConfig) -> Arc<Self> {
        let b = Arc::new(Self {
            cfg,
            grant: Mutex::new(None),
        });
        let weak = Arc::downgrade(&b);
        std::thread::Builder::new()
            .name("grant-watchdog".into())
            .spawn(move || {
                while let Some(b) = weak.upgrade() {
                    b.sweep();
                    drop(b);
                    std::thread::sleep(Duration::from_secs(1));
                }
            })
            .expect("spawn watchdog");
        b
    }

    /// Ends grants that lapsed or whose agent died.
    fn sweep(&self) {
        let dead = {
            let g = lock(&self.grant);
            g.as_ref().is_some_and(|gr| {
                lock(&gr.last_beat).elapsed() > Duration::from_secs(GRANT_LAPSE_SECS)
                    || !lock(&gr.agent.child).is_alive()
            })
        };
        if dead {
            tracing::warn!(target: "security", "elevated grant lapsed or its agent stopped; revoking");
            self.end_grant();
        }
    }

    fn end_grant(&self) {
        if let Some(g) = lock(&self.grant).take() {
            tracing::info!(target: "security", label = %g.label, "elevated control revoked");
            g.agent.shutdown();
        }
    }

    // ---- caller checks -------------------------------------------------------------------------

    /// Is `pid` (in `session`) the installed RemoteX application in the active console session?
    pub fn caller_allowed(&self, pid: u32, session: u32) -> bool {
        let Ok(image) = pipe::process_image(pid) else {
            return false;
        };
        if !self.cfg.allowed_ui.iter().any(|p| same_path(p, &image)) {
            tracing::warn!(target: "security", pid, image = %image.display(), "pipe caller is not the RemoteX application");
            return false;
        }
        if self.cfg.enforce_console_session {
            // SAFETY: no arguments; returns 0xFFFFFFFF when there is no console session.
            let active = unsafe { WTSGetActiveConsoleSessionId() };
            if active == u32::MAX || active != session {
                tracing::warn!(target: "security", pid, session, active, "caller is not in the active console session");
                return false;
            }
        }
        true
    }

    fn install_is_protected(&self) -> bool {
        if !self.cfg.require_protected_install {
            return true;
        }
        // Program Files is writable only by administrators and TrustedInstaller by default. A per-user
        // location would let any local process replace the executable this service trusts.
        let dir = self
            .cfg
            .exe
            .parent()
            .map(|p| p.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        ["programfiles", "programfiles(x86)", "programw6432"]
            .iter()
            .filter_map(|v| std::env::var(v).ok())
            .any(|pf| dir.starts_with(&pf.to_lowercase()))
    }

    // ---- grant lifecycle -------------------------------------------------------------------------

    pub fn grant(&self, pid: u32, label: &str) -> Result<[u8; 16], DenyReason> {
        if !self.install_is_protected() {
            tracing::error!(target: "security", "refusing elevated control: installation directory is not protected");
            return Err(DenyReason::NotAuthorized);
        }
        {
            let g = lock(&self.grant);
            if g.is_some() {
                return Err(DenyReason::AlreadyGranted);
            }
        }
        let agent = self.start_agent().map_err(|e| {
            tracing::error!(target: "security", error = %e, "could not start the elevated helper");
            DenyReason::AgentUnavailable
        })?;
        let token = remotex_security::ids::random_bytes::<16>();
        let mut slot = lock(&self.grant);
        if slot.is_some() {
            agent.shutdown();
            return Err(DenyReason::AlreadyGranted);
        }
        tracing::info!(target: "security", pid, %label, "elevated control granted by the owner of this PC");
        *slot = Some(Arc::new(Grant {
            token,
            owner_pid: pid,
            label: label.chars().take(64).collect(),
            last_beat: Mutex::new(Instant::now()),
            agent,
        }));
        Ok(token)
    }

    /// The grant `token` names, if it is live and belongs to `pid`.
    fn current(&self, token: &[u8; 16], pid: u32) -> Result<Arc<Grant>, DenyReason> {
        let g = lock(&self.grant).clone().ok_or(DenyReason::NotGranted)?;
        // Constant-time enough for a 128-bit random capability that never leaves this machine.
        if g.token != *token || g.owner_pid != pid {
            return Err(DenyReason::NotAuthorized);
        }
        Ok(g)
    }

    pub fn attach(&self, token: &[u8; 16], pid: u32) -> Result<(), DenyReason> {
        self.current(token, pid).map(|_| ())
    }

    pub fn revoke(&self, token: &[u8; 16], pid: u32) {
        if self.current(token, pid).is_ok() {
            self.end_grant();
        }
    }

    /// The owner's connection dropped: the grant ends with it.
    pub fn owner_gone(&self, pid: u32) {
        let owned = lock(&self.grant).as_ref().is_some_and(|g| g.owner_pid == pid);
        if owned {
            self.end_grant();
        }
    }

    pub fn heartbeat(&self, token: &[u8; 16], pid: u32) -> Result<(), DenyReason> {
        let g = self.current(token, pid)?;
        *lock(&g.last_beat) = Instant::now();
        Ok(())
    }

    // ---- relayed operations ----------------------------------------------------------------------

    pub fn inject(
        &self,
        token: &[u8; 16],
        pid: u32,
        ev: InputEvent,
        display: DisplayInfo,
    ) -> Result<(), DenyReason> {
        let g = self.current(token, pid)?;
        let mut f = lock(&g.agent.input);
        write_msg(&mut *f, &ServiceToAgent::Inject { ev, display }).map_err(|_| DenyReason::AgentUnavailable)
    }

    pub fn capture(
        &self,
        token: &[u8; 16],
        pid: u32,
        display: DisplayInfo,
        out_w: u32,
        out_h: u32,
    ) -> Result<Option<Picture>, DenyReason> {
        let g = self.current(token, pid)?;
        let mut f = lock(&g.agent.video);
        write_msg(
            &mut *f,
            &ServiceToAgent::Capture {
                display,
                out_w,
                out_h,
            },
        )
        .map_err(|_| DenyReason::AgentUnavailable)?;
        match read_msg::<_, AgentToService>(&mut *f) {
            Ok(AgentToService::Picture(p)) => Ok(Some(p)),
            Ok(AgentToService::NoPicture) => Ok(None),
            _ => Err(DenyReason::AgentUnavailable),
        }
    }

    pub fn status(&self, token: &[u8; 16], pid: u32) -> Result<bool, DenyReason> {
        let g = self.current(token, pid)?;
        let mut f = lock(&g.agent.video);
        write_msg(&mut *f, &ServiceToAgent::Status).map_err(|_| DenyReason::AgentUnavailable)?;
        match read_msg::<_, AgentToService>(&mut *f) {
            Ok(AgentToService::Status { secure_desktop }) => Ok(secure_desktop),
            _ => Err(DenyReason::AgentUnavailable),
        }
    }

    pub fn has_grant(&self) -> bool {
        lock(&self.grant).is_some()
    }

    // ---- agent startup ---------------------------------------------------------------------------

    fn start_agent(&self) -> std::io::Result<Agent> {
        let tag = remotex_security::ids::random_bytes::<8>();
        let hex: String = tag.iter().map(|b| format!("{b:02x}")).collect();
        let (n_in, n_vid) = (
            format!(r"\\.\pipe\remotex-agent-{hex}-in"),
            format!(r"\\.\pipe\remotex-agent-{hex}-vid"),
        );
        // Only SYSTEM (or, in development, the same user) can ever open these pipes.
        let sddl = match self.cfg.launch {
            LaunchKind::System => pipe::SYSTEM_ONLY_SDDL,
            LaunchKind::SameUser => pipe::USER_PIPE_SDDL,
        };
        let sec = PipeSecurity::from_sddl(sddl)?;
        let in_srv = pipe::create_server(&n_in, &sec, true)?;
        let vid_srv = pipe::create_server(&n_vid, &sec, true)?;
        let mode = match self.cfg.launch {
            // SAFETY: no arguments.
            LaunchKind::System => LaunchMode::SystemInSession(unsafe { WTSGetActiveConsoleSessionId() }),
            LaunchKind::SameUser => LaunchMode::SameUser,
        };
        let mut child = spawn_agent(&self.cfg.exe, &n_in, &n_vid, mode)?;

        let accept_with_timeout = |server: File, name: String| -> std::io::Result<File> {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let r = pipe::accept(&server).map(|_| server);
                let _ = tx.send(r);
            });
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(r) => r,
                Err(_) => {
                    // Unblock the waiting accept so its thread ends.
                    let _ = pipe::connect(&name, Duration::from_millis(100));
                    Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "agent did not connect",
                    ))
                }
            }
        };
        let connected = accept_with_timeout(in_srv, n_in.clone())
            .and_then(|i| accept_with_timeout(vid_srv, n_vid.clone()).map(|v| (i, v)));
        let (mut input, mut video) = match connected {
            Ok(x) => x,
            Err(e) => {
                child.kill();
                return Err(e);
            }
        };
        // The agent must be exactly the process we started, and say so on the right pipe.
        for (conn, role) in [(&mut input, ConnRole::Input), (&mut video, ConnRole::Video)] {
            let ok = pipe::client_pid(conn).is_ok_and(|p| p == child.pid())
                && matches!(read_msg::<_, AgentToService>(conn), Ok(AgentToService::Ready { pid, role: r }) if pid == child.pid() && r == role);
            if !ok {
                child.kill();
                return Err(std::io::Error::other("agent identity check failed"));
            }
        }
        Ok(Agent {
            input: Mutex::new(input),
            video: Mutex::new(video),
            child: Mutex::new(child),
        })
    }
}
