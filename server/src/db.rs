//! SQLite persistence. Only operational metadata is stored: never screen, clipboard or file content.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS devices (
    id TEXT PRIMARY KEY,
    secret_hash BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    client_version TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    viewer_id TEXT NOT NULL,
    host_id TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    mode TEXT NOT NULL DEFAULT 'pending',
    relay_bytes INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS authentication_attempts (
    ts INTEGER NOT NULL,
    ip TEXT NOT NULL,
    kind TEXT NOT NULL,
    success INTEGER NOT NULL,
    device_id TEXT
);
CREATE INDEX IF NOT EXISTS idx_attempts_ts ON authentication_attempts(ts);
CREATE TABLE IF NOT EXISTS server_nodes (
    id TEXT PRIMARY KEY,
    started_at INTEGER NOT NULL,
    last_heartbeat INTEGER NOT NULL,
    version TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS software_versions (
    version TEXT PRIMARY KEY,
    first_seen INTEGER NOT NULL
);
";

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Runs a query on the blocking pool so slow disk never stalls the async reactor.
    pub async fn call<R, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&Connection) -> rusqlite::Result<R> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn.lock().map_err(|_| anyhow!("database mutex poisoned"))?;
            f(&guard).map_err(anyhow::Error::from)
        })
        .await?
    }

    pub async fn insert_device(&self, id: String, hash: Vec<u8>) -> Result<bool> {
        self.call(move |c| {
            let n = c.execute(
                "INSERT OR IGNORE INTO devices (id, secret_hash, created_at, last_seen) VALUES (?1, ?2, ?3, ?3)",
                params![id, hash, now()],
            )?;
            Ok(n == 1)
        })
        .await
    }

    pub async fn device_hash(&self, id: String) -> Result<Option<Vec<u8>>> {
        self.call(move |c| {
            c.query_row(
                "SELECT secret_hash FROM devices WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
        })
        .await
    }

    pub async fn touch_device(&self, id: String, version: String) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "UPDATE devices SET last_seen = ?2, client_version = ?3 WHERE id = ?1",
                params![id, now(), version],
            )?;
            c.execute(
                "INSERT OR IGNORE INTO software_versions (version, first_seen) VALUES (?1, ?2)",
                params![version, now()],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn log_attempt(&self, ip: String, kind: &'static str, success: bool, device: Option<String>) {
        let _ = self
            .call(move |c| {
                c.execute(
                    "INSERT INTO authentication_attempts (ts, ip, kind, success, device_id) VALUES (?1,?2,?3,?4,?5)",
                    params![now(), ip, kind, success as i64, device],
                )
            })
            .await;
    }

    pub async fn session_start(&self, id: String, viewer: String, host: String) {
        let _ = self
            .call(move |c| {
                c.execute(
                    "INSERT OR IGNORE INTO sessions (id, viewer_id, host_id, started_at) VALUES (?1,?2,?3,?4)",
                    params![id, viewer, host, now()],
                )
            })
            .await;
    }

    pub async fn session_end(&self, id: String, mode: &'static str, bytes: u64) {
        let _ = self
            .call(move |c| {
                c.execute(
                    "UPDATE sessions SET ended_at = ?2, mode = ?3, relay_bytes = ?4 WHERE id = ?1",
                    params![id, now(), mode, bytes as i64],
                )
            })
            .await;
    }

    pub async fn version_counts(&self) -> Vec<(String, i64)> {
        self.call(|c| {
            let mut stmt = c.prepare(
                "SELECT client_version, COUNT(*) FROM devices WHERE client_version != '' GROUP BY client_version ORDER BY 2 DESC LIMIT 20",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
        .await
        .unwrap_or_default()
    }

    pub async fn recent_failures(&self, since: i64) -> i64 {
        self.call(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM authentication_attempts WHERE success = 0 AND ts >= ?1",
                params![since],
                |r| r.get(0),
            )
        })
        .await
        .unwrap_or(0)
    }

    pub async fn heartbeat(&self, node: String, started: i64, version: String) {
        let _ = self
            .call(move |c| {
                c.execute(
                    "INSERT INTO server_nodes (id, started_at, last_heartbeat, version) VALUES (?1,?2,?3,?4)
                     ON CONFLICT(id) DO UPDATE SET last_heartbeat = ?3, started_at = ?2, version = ?4",
                    params![node, started, now(), version],
                )
            })
            .await;
    }

    /// Retention: attempt log 30 days, finished sessions 90 days.
    pub async fn purge_old(&self) {
        let _ = self
            .call(|c| {
                c.execute(
                    "DELETE FROM authentication_attempts WHERE ts < ?1",
                    params![now() - 30 * 86400],
                )?;
                c.execute(
                    "DELETE FROM sessions WHERE ended_at IS NOT NULL AND ended_at < ?1",
                    params![now() - 90 * 86400],
                )
            })
            .await;
    }
}
