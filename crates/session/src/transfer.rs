//! File transfer over an established session: offers, resumable chunked streaming, verification,
//! pause/cancel/retry, and (host side) directory browsing. All peer input is treated as hostile.

use crate::types::{Event, EventTx, TransferInfo, TransferStatus};
use remotex_common::peer::{DirEntry, FileMsg, Msg};
use remotex_files::{expand_paths, hash_file, part_path, sanitize_relative, IncomingFile, OutgoingFile};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use crate::secure::SecureTx;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Host,
    Viewer,
}

#[derive(Clone)]
pub struct FilePolicy {
    pub side: Side,
    pub download_dir: PathBuf,
    pub ask_before_receiving: bool,
    pub max_file_size: u64,
    pub max_active: usize,
}

struct Ctl {
    paused: AtomicBool,
    cancelled: AtomicBool,
}

struct Speed {
    last_emit: Instant,
    window: Vec<(Instant, u64)>,
}

struct Transfer {
    info: TransferInfo,
    path: PathBuf,
    ctl: Arc<Ctl>,
    incoming: Option<IncomingFile>,
    speed: Speed,
    /// Waiting for local approval or a free slot.
    queued: bool,
    /// Sender: accepted by the peer and streaming.
    resume_from: u64,
}

pub struct FileManager {
    policy: FilePolicy,
    session: u64,
    tx: SecureTx,
    events: EventTx,
    allowed: AtomicBool,
    next: AtomicU64,
    transfers: Mutex<HashMap<u64, Transfer>>,
    /// Viewer side: host-initiated offers are only honoured shortly after we asked for a download.
    accept_offers_until: Mutex<Option<Instant>>,
    pending_approval: Mutex<HashMap<u64, (String, u64)>>,
}

const MAX_LIST: usize = 5000;

impl FileManager {
    pub fn new(policy: FilePolicy, session: u64, tx: SecureTx, events: EventTx, allowed: bool) -> Arc<Self> {
        Arc::new(Self {
            policy,
            session,
            tx,
            events,
            allowed: AtomicBool::new(allowed),
            next: AtomicU64::new(0),
            transfers: Mutex::new(HashMap::new()),
            accept_offers_until: Mutex::new(None),
            pending_approval: Mutex::new(HashMap::new()),
        })
    }

    pub fn set_allowed(&self, allowed: bool) {
        self.allowed.store(allowed, Ordering::SeqCst);
        if !allowed {
            self.cancel_all();
        }
    }

    fn allowed(&self) -> bool {
        self.allowed.load(Ordering::SeqCst)
    }

    /// Ids are namespaced by side so both peers can allocate without coordination.
    fn alloc_id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed) * 2 + u64::from(self.policy.side == Side::Viewer)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Transfer>> {
        self.transfers.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn emit(&self, info: TransferInfo) {
        let _ = self.events.send(Event::Transfer {
            session: self.session,
            info,
        });
    }

    fn update(&self, id: u64, f: impl FnOnce(&mut Transfer)) {
        let info = {
            let mut g = self.lock();
            let Some(t) = g.get_mut(&id) else { return };
            f(t);
            t.info.clone()
        };
        self.emit(info);
    }

    fn fail(&self, id: u64, why: &str) {
        self.update(id, |t| {
            t.info.status = TransferStatus::Failed;
            t.info.error = Some(why.to_string());
            t.info.speed_bps = 0;
            t.info.eta_secs = None;
            t.incoming = None;
        });
    }

    fn track(&self, id: u64, name: &str, size: u64, upload: bool, path: PathBuf, status: TransferStatus) {
        let now = Instant::now();
        let t = Transfer {
            info: TransferInfo {
                id,
                name: name.to_string(),
                size,
                transferred: 0,
                upload,
                status,
                speed_bps: 0,
                eta_secs: None,
                error: None,
            },
            path,
            ctl: Arc::new(Ctl {
                paused: AtomicBool::new(false),
                cancelled: AtomicBool::new(false),
            }),
            incoming: None,
            speed: Speed {
                last_emit: now,
                window: Vec::new(),
            },
            queued: false,
            resume_from: 0,
        };
        let info = t.info.clone();
        self.lock().insert(id, t);
        self.emit(info);
    }

    fn active_count(&self) -> usize {
        self.lock()
            .values()
            .filter(|t| t.info.status == TransferStatus::Active)
            .count()
    }

    fn progress(&self, id: u64, transferred: u64) {
        let emit = {
            let mut g = self.lock();
            let Some(t) = g.get_mut(&id) else { return };
            let now = Instant::now();
            t.info.transferred = transferred;
            t.speed.window.push((now, transferred));
            t.speed
                .window
                .retain(|(at, _)| now.duration_since(*at) < Duration::from_secs(3));
            if let (Some(first), Some(last)) = (t.speed.window.first(), t.speed.window.last()) {
                let dt = last.0.duration_since(first.0).as_secs_f64();
                if dt > 0.2 {
                    t.info.speed_bps = ((last.1 - first.1) as f64 / dt) as u64;
                }
            }
            t.info.eta_secs =
                (t.info.speed_bps > 0).then(|| t.info.size.saturating_sub(transferred) / t.info.speed_bps);
            if now.duration_since(t.speed.last_emit) > Duration::from_millis(250) {
                t.speed.last_emit = now;
                Some(t.info.clone())
            } else {
                None
            }
        };
        if let Some(info) = emit {
            self.emit(info);
        }
    }

    // ---- local commands ------------------------------------------------------------------------

    /// Queues files and folders for upload to the peer.
    pub fn send_paths(self: &Arc<Self>, paths: Vec<PathBuf>) -> Result<usize, String> {
        if !self.allowed() {
            return Err("file transfer is not permitted in this session".into());
        }
        let files = expand_paths(&paths).map_err(|e| e.to_string())?;
        let count = files.len();
        for (path, name) in files {
            self.offer(path, name)?;
        }
        Ok(count)
    }

    fn offer(self: &Arc<Self>, path: PathBuf, name: String) -> Result<(), String> {
        let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
        let id = self.alloc_id();
        self.track(id, &name, size, true, path, TransferStatus::Pending);
        self.tx.send(Msg::File(FileMsg::Offer { id, name, size }));
        Ok(())
    }

    /// Viewer: request a file or folder from the host.
    pub fn download(&self, remote_path: String) -> Result<(), String> {
        if !self.allowed() || self.policy.side != Side::Viewer {
            return Err("downloads are not permitted".into());
        }
        *self.accept_offers_until.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(Instant::now() + Duration::from_secs(30));
        let id = self.alloc_id();
        self.tx.send(Msg::File(FileMsg::Fetch {
            id,
            path: remote_path,
        }));
        Ok(())
    }

    pub fn list_dir(&self, path: String) {
        self.tx.send(Msg::File(FileMsg::ListDir { path }));
    }

    pub fn pause(&self, id: u64) {
        if let Some(t) = self.lock().get(&id) {
            t.ctl.paused.store(true, Ordering::SeqCst);
        }
        self.update(id, |t| {
            if t.info.status == TransferStatus::Active {
                t.info.status = TransferStatus::Paused;
                t.info.speed_bps = 0;
            }
        });
    }

    pub fn resume(&self, id: u64) {
        if let Some(t) = self.lock().get(&id) {
            t.ctl.paused.store(false, Ordering::SeqCst);
        }
        self.update(id, |t| {
            if t.info.status == TransferStatus::Paused {
                t.info.status = TransferStatus::Active;
            }
        });
    }

    pub fn cancel(&self, id: u64) {
        let known = {
            let g = self.lock();
            g.get(&id)
                .map(|t| t.ctl.cancelled.store(true, Ordering::SeqCst))
                .is_some()
        };
        if known {
            self.tx.send(Msg::File(FileMsg::Cancel { id }));
            self.update(id, |t| {
                t.info.status = TransferStatus::Cancelled;
                t.info.speed_bps = 0;
                t.incoming = None; // keep the .part so it can be resumed
            });
        }
    }

    fn cancel_all(&self) {
        let ids: Vec<u64> = self
            .lock()
            .iter()
            .filter(|(_, t)| {
                matches!(
                    t.info.status,
                    TransferStatus::Active | TransferStatus::Pending | TransferStatus::Paused
                )
            })
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.cancel(id);
        }
    }

    /// Retries a failed or cancelled upload, resuming from what the peer already has.
    pub fn retry(self: &Arc<Self>, id: u64) -> Result<(), String> {
        let (path, name, upload) = {
            let g = self.lock();
            let t = g.get(&id).ok_or("unknown transfer")?;
            (t.path.clone(), t.info.name.clone(), t.info.upload)
        };
        if !upload {
            return Err("only uploads can be retried from this side; ask the sender to retry".into());
        }
        self.lock().remove(&id);
        self.offer(path, name)
    }

    /// Host: answer to a `FileRequest` event.
    pub fn respond(self: &Arc<Self>, id: u64, accept: bool) {
        let Some((name, size)) = self
            .pending_approval
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id)
        else {
            return;
        };
        if accept {
            self.accept_incoming(id, &name, size);
        } else {
            self.tx.send(Msg::File(FileMsg::Reject {
                id,
                reason: "declined".into(),
            }));
        }
    }

    // ---- peer messages -------------------------------------------------------------------------

    pub async fn on_msg(self: &Arc<Self>, msg: FileMsg) {
        if !self.allowed() {
            if let FileMsg::Offer { id, .. } | FileMsg::Fetch { id, .. } = &msg {
                self.tx.send(Msg::File(FileMsg::Reject {
                    id: *id,
                    reason: "file transfer disabled".into(),
                }));
            }
            return;
        }
        match msg {
            FileMsg::Offer { id, name, size } => self.on_offer(id, name, size),
            FileMsg::Accept { id, resume_from } => self.on_accept(id, resume_from),
            FileMsg::Reject { id, reason } => self.fail(id, &reason),
            FileMsg::Chunk { id, offset, data } => self.on_chunk(id, offset, &data),
            FileMsg::Done { id, sha256 } => self.on_done(id, sha256).await,
            FileMsg::Verified { id, ok } => {
                if ok {
                    self.update(id, |t| {
                        t.info.status = TransferStatus::Completed;
                        t.info.transferred = t.info.size;
                        t.info.speed_bps = 0;
                        t.info.eta_secs = None;
                    });
                } else {
                    self.fail(id, "verification failed on the receiving side");
                }
            }
            FileMsg::Cancel { id } => {
                if let Some(t) = self.lock().get(&id) {
                    t.ctl.cancelled.store(true, Ordering::SeqCst);
                }
                self.update(id, |t| {
                    t.info.status = TransferStatus::Cancelled;
                    t.info.speed_bps = 0;
                    t.incoming = None;
                });
            }
            FileMsg::ListDir { path } => self.on_list(path),
            FileMsg::DirListing { path, entries, error } => {
                let _ = self.events.send(Event::DirListing {
                    session: self.session,
                    path,
                    entries,
                    error,
                });
            }
            FileMsg::Fetch { id, path } => self.on_fetch(id, path),
        }
    }

    fn on_offer(self: &Arc<Self>, id: u64, name: String, size: u64) {
        let from_peer_namespace = id % 2 == u64::from(self.policy.side == Side::Host);
        if !from_peer_namespace || self.lock().contains_key(&id) {
            self.tx.send(Msg::File(FileMsg::Reject {
                id,
                reason: "invalid transfer id".into(),
            }));
            return;
        }
        if sanitize_relative(&name).is_err() || size > self.policy.max_file_size {
            self.tx.send(Msg::File(FileMsg::Reject {
                id,
                reason: "file not acceptable".into(),
            }));
            return;
        }
        match self.policy.side {
            Side::Viewer => {
                let ok = self
                    .accept_offers_until
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .is_some_and(|until| Instant::now() < until);
                if !ok {
                    self.tx.send(Msg::File(FileMsg::Reject {
                        id,
                        reason: "unsolicited transfer".into(),
                    }));
                    return;
                }
                self.accept_incoming(id, &name, size);
            }
            Side::Host if self.policy.ask_before_receiving => {
                self.pending_approval
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(id, (name.clone(), size));
                let _ = self.events.send(Event::FileRequest {
                    session: self.session,
                    id,
                    name,
                    size,
                });
            }
            Side::Host => self.accept_incoming(id, &name, size),
        }
    }

    fn accept_incoming(self: &Arc<Self>, id: u64, name: &str, size: u64) {
        if self.active_count() >= self.policy.max_active {
            // Queue rather than refuse: acceptance is sent when a slot frees up (checked on completion).
            self.track(id, name, size, false, PathBuf::new(), TransferStatus::Pending);
            if let Some(t) = self.lock().get_mut(&id) {
                t.queued = true;
            }
            return;
        }
        self.start_incoming(id, name, size);
    }

    fn start_incoming(self: &Arc<Self>, id: u64, name: &str, size: u64) {
        let resume = sanitize_relative(name)
            .map(|rel| {
                let target = self.policy.download_dir.join(rel);
                part_path(&target).exists() && !target.exists()
            })
            .unwrap_or(false);
        match IncomingFile::open(
            &self.policy.download_dir,
            name,
            size,
            self.policy.max_file_size,
            resume,
        ) {
            Ok(file) => {
                let resume_from = file.received;
                if !self.lock().contains_key(&id) {
                    self.track(
                        id,
                        name,
                        size,
                        false,
                        self.policy.download_dir.join(name),
                        TransferStatus::Active,
                    );
                }
                self.update(id, |t| {
                    t.info.status = TransferStatus::Active;
                    t.info.transferred = resume_from;
                    t.queued = false;
                    t.incoming = Some(file);
                });
                self.tx.send(Msg::File(FileMsg::Accept { id, resume_from }));
            }
            Err(e) => {
                self.tx.send(Msg::File(FileMsg::Reject {
                    id,
                    reason: e.to_string(),
                }));
                self.fail(id, &e.to_string());
            }
        }
    }

    fn start_queued(self: &Arc<Self>) {
        while self.active_count() < self.policy.max_active {
            let next = self
                .lock()
                .iter()
                .find(|(_, t)| t.queued && !t.info.upload)
                .map(|(id, t)| (*id, t.info.name.clone(), t.info.size));
            let Some((id, name, size)) = next else { break };
            self.start_incoming(id, &name, size);
        }
    }

    fn on_accept(self: &Arc<Self>, id: u64, resume_from: u64) {
        let (path, ctl, size) = {
            let mut g = self.lock();
            let Some(t) = g
                .get_mut(&id)
                .filter(|t| t.info.upload && t.info.status == TransferStatus::Pending)
            else {
                return;
            };
            t.info.status = TransferStatus::Active;
            t.resume_from = resume_from;
            (t.path.clone(), t.ctl.clone(), t.info.size)
        };
        if resume_from > size {
            self.fail(id, "peer requested an invalid resume offset");
            return;
        }
        self.update(id, |t| t.info.transferred = resume_from);
        let me = self.clone();
        tokio::spawn(async move { me.stream_file(id, path, resume_from, ctl).await });
    }

    async fn stream_file(self: Arc<Self>, id: u64, path: PathBuf, resume_from: u64, ctl: Arc<Ctl>) {
        let open = tokio::task::spawn_blocking({
            let path = path.clone();
            move || OutgoingFile::open(&path).and_then(|mut f| f.seek_to(resume_from).map(|_| f))
        })
        .await;
        let mut file = match open {
            Ok(Ok(f)) => f,
            _ => return self.fail(id, "could not read the file"),
        };
        loop {
            if ctl.cancelled.load(Ordering::SeqCst) {
                return;
            }
            while ctl.paused.load(Ordering::SeqCst) && !ctl.cancelled.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let read = tokio::task::spawn_blocking(move || {
                let r = file.next_chunk();
                (file, r)
            })
            .await;
            let (f, chunk) = match read {
                Ok(v) => v,
                Err(_) => return self.fail(id, "read task failed"),
            };
            file = f;
            match chunk {
                Ok(Some((offset, data))) => {
                    let end = offset + data.len() as u64;
                    if !self
                        .tx
                        .send_bulk(Msg::File(FileMsg::Chunk { id, offset, data }))
                        .await
                    {
                        return self.fail(id, "connection closed");
                    }
                    self.progress(id, end);
                }
                Ok(None) => break,
                Err(e) => return self.fail(id, &e.to_string()),
            }
        }
        let hash = tokio::task::spawn_blocking(move || hash_file(&path)).await;
        match hash {
            // Must share the bulk queue with the chunks, or it would overtake the tail of the file.
            Ok(Ok(sha256)) => {
                self.tx.send_bulk(Msg::File(FileMsg::Done { id, sha256 })).await;
            }
            _ => self.fail(id, "could not hash the file"),
        }
    }

    fn on_chunk(self: &Arc<Self>, id: u64, offset: u64, data: &[u8]) {
        let result = {
            let mut g = self.lock();
            let Some(t) = g.get_mut(&id) else { return };
            if t.ctl.cancelled.load(Ordering::SeqCst) {
                return;
            }
            match t.incoming.as_mut() {
                Some(inc) => inc.write_chunk(offset, data).map(|_| inc.received),
                None => return,
            }
        };
        match result {
            Ok(received) => self.progress(id, received),
            Err(e) => {
                self.tx.send(Msg::File(FileMsg::Cancel { id }));
                self.fail(id, &e.to_string());
            }
        }
    }

    async fn on_done(self: &Arc<Self>, id: u64, sha256: [u8; 32]) {
        let incoming = self.lock().get_mut(&id).and_then(|t| t.incoming.take());
        let Some(incoming) = incoming else { return };
        let name = self
            .lock()
            .get(&id)
            .map(|t| t.info.name.clone())
            .unwrap_or_default();
        let result = tokio::task::spawn_blocking(move || incoming.finish(&sha256)).await;
        match result {
            Ok(Ok(final_path)) => {
                self.tx.send(Msg::File(FileMsg::Verified { id, ok: true }));
                self.update(id, |t| {
                    t.info.status = TransferStatus::Completed;
                    t.info.transferred = t.info.size;
                    t.info.speed_bps = 0;
                    t.info.eta_secs = None;
                    t.path = final_path;
                });
                let _ = name;
            }
            _ => {
                self.tx.send(Msg::File(FileMsg::Verified { id, ok: false }));
                self.fail(id, "integrity check failed");
            }
        }
        self.start_queued();
    }

    fn on_list(&self, path: String) {
        if self.policy.side != Side::Host {
            return;
        }
        let (entries, error) = list_directory(&path);
        self.tx
            .send(Msg::File(FileMsg::DirListing { path, entries, error }));
    }

    fn on_fetch(self: &Arc<Self>, req_id: u64, path: String) {
        if self.policy.side != Side::Host {
            self.tx.send(Msg::File(FileMsg::Reject {
                id: req_id,
                reason: "not supported".into(),
            }));
            return;
        }
        let real = match std::fs::canonicalize(&path) {
            Ok(p) => p,
            Err(_) => {
                self.tx.send(Msg::File(FileMsg::Reject {
                    id: req_id,
                    reason: "not found".into(),
                }));
                return;
            }
        };
        match expand_paths(&[real]) {
            Ok(files) if !files.is_empty() => {
                for (p, name) in files {
                    if let Err(e) = self.offer(p, name) {
                        tracing::debug!(error = %e, "could not offer file");
                    }
                }
            }
            _ => {
                self.tx.send(Msg::File(FileMsg::Reject {
                    id: req_id,
                    reason: "nothing to send".into(),
                }));
            }
        }
    }
}

/// Lists a directory (or the drive roots for an empty path). Sizes are file sizes; folders report 0.
pub fn list_directory(path: &str) -> (Vec<DirEntry>, Option<String>) {
    if path.is_empty() {
        let roots: Vec<DirEntry> = (b'A'..=b'Z')
            .map(|c| format!("{}:\\", c as char))
            .filter(|r| Path::new(r).exists())
            .map(|name| DirEntry {
                name,
                is_dir: true,
                size: 0,
            })
            .collect();
        return (roots, None);
    }
    match std::fs::read_dir(path) {
        Ok(rd) => {
            let mut entries: Vec<DirEntry> = rd
                .flatten()
                .take(MAX_LIST)
                .filter_map(|e| {
                    let meta = e.metadata().ok()?;
                    Some(DirEntry {
                        name: e.file_name().to_string_lossy().into_owned(),
                        is_dir: meta.is_dir(),
                        size: if meta.is_dir() { 0 } else { meta.len() },
                    })
                })
                .collect();
            entries.sort_by(|a, b| {
                b.is_dir
                    .cmp(&a.is_dir)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            (entries, None)
        }
        Err(e) => (Vec::new(), Some(e.to_string())),
    }
}
