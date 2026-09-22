//! RemoteX server: device registry, signaling, encrypted-traffic relay and health endpoints.

pub mod admin;
pub mod config;
pub mod db;
pub mod hub;
pub mod logging;
pub mod tls;
pub mod ws;

use anyhow::Result;
use axum::{routing::get, Router};
use config::Config;
use hub::Hub;
use std::{
    net::{SocketAddr, TcpListener},
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tower_http::services::ServeDir;
use tracing::info;
use ws::AppState;

pub fn build_app(state: AppState) -> Router {
    Router::new()
        .route("/ws", get(ws::ws_handler))
        .route("/health", get(admin::health))
        .route("/admin", get(admin::page))
        .route("/admin/api/stats", get(admin::stats))
        .nest_service("/updates", ServeDir::new(state.hub.cfg.updates_dir.clone()))
        .with_state(state)
}

/// Background upkeep: abandoned-session cleanup, system metrics, node heartbeat, retention.
fn spawn_housekeeping(state: AppState) {
    tokio::spawn(async move {
        let mut sys = System::new();
        let pid = Pid::from_u32(std::process::id());
        let started = db::now();
        let mut last_bytes = 0u64;
        let mut tick = 0u64;
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            tick += 1;

            for e in state.hub.sweep() {
                e.notify();
                info!(target: "connection", session = %e.session.to_hex(), "session expired");
                state
                    .db
                    .session_end(e.session.to_hex(), e.mode.as_str(), e.bytes)
                    .await;
            }

            let total = state.hub.metrics.relay_bytes.load(Ordering::Relaxed);
            state
                .hub
                .metrics
                .relay_bps
                .store((total - last_bytes) / 5, Ordering::Relaxed);
            last_bytes = total;

            sys.refresh_cpu_usage();
            sys.refresh_memory();
            sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
            if let Ok(mut snap) = state.hub.sys.lock() {
                snap.cpu_percent = sys.global_cpu_usage();
                snap.mem_used = sys.used_memory();
                snap.mem_total = sys.total_memory();
                snap.process_mem = sys.process(pid).map(|p| p.memory()).unwrap_or(0);
            }

            if tick.is_multiple_of(12) {
                state
                    .db
                    .heartbeat("primary".into(), started, env!("CARGO_PKG_VERSION").into())
                    .await;
            }
            if tick % 720 == 1 {
                state.db.purge_old().await;
            }
        }
    });
}

/// Serves until `shutdown` resolves. The listener is supplied so tests can bind port 0.
pub async fn serve(
    cfg: Arc<Config>,
    listener: TcpListener,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    listener.set_nonblocking(true)?;
    let db = db::Db::open(&cfg.database_path)?;
    let hub = Hub::new(cfg.clone());
    let closing = hub.shutdown.clone();
    let state = AppState { hub, db };
    spawn_housekeeping(state.clone());

    let app = build_app(state).into_make_service_with_connect_info::<SocketAddr>();
    let handle = axum_server::Handle::new();
    let h = handle.clone();
    tokio::spawn(async move {
        shutdown.await;
        // Tell every client the service is going away, then give connections a moment to close.
        let _ = closing.send(true);
        h.graceful_shutdown(Some(Duration::from_secs(5)));
    });

    match &cfg.tls {
        Some((cert, key)) => {
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
            info!("listening on {} (TLS)", listener.local_addr()?);
            axum_server::from_tcp_rustls(listener, tls)
                .handle(handle)
                .serve(app)
                .await?;
        }
        None => {
            info!(
                "listening on {} (plain HTTP/WS; development only)",
                listener.local_addr()?
            );
            axum_server::from_tcp(listener)
                .acceptor(axum_server::accept::NoDelayAcceptor::new())
                .handle(handle)
                .serve(app)
                .await?;
        }
    }
    Ok(())
}
