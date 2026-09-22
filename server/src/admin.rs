//! Infrastructure health only: counters and versions. No session content exists on the server to expose.

use crate::ws::AppState;
use axum::{
    extract::{ConnectInfo, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json},
};
use serde_json::json;
use std::{net::SocketAddr, sync::atomic::Ordering};
use subtle::ConstantTimeEq;

pub async fn health(State(st): State<AppState>) -> Json<serde_json::Value> {
    Json(
        json!({ "ok": true, "version": env!("CARGO_PKG_VERSION"), "uptime_secs": st.hub.started.elapsed().as_secs() }),
    )
}

pub async fn page() -> Html<&'static str> {
    Html(include_str!("admin.html"))
}

pub async fn stats(
    State(st): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let ip = addr.ip().to_canonical();
    if st.hub.admin_locked(ip) {
        return (StatusCode::TOO_MANY_REQUESTS, Json(json!({ "error": "locked" }))).into_response();
    }
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let ok: bool = given.as_bytes().ct_eq(st.hub.cfg.admin_token.as_bytes()).into();
    if !ok {
        st.hub.admin_failure(ip);
        tracing::warn!(target: "security", %ip, "admin authentication failed");
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "unauthorized" }))).into_response();
    }

    let s = st.hub.stats();
    let m = &st.hub.metrics;
    let sys = st.hub.sys.lock().map(|g| g.clone()).unwrap_or_default();
    let since = crate::db::now() - 3600;
    Json(json!({
        "server_version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": st.hub.started.elapsed().as_secs(),
        "online_devices": s.online,
        "sessions": { "active": s.sessions, "direct": s.direct, "relay": s.relay, "pending": s.pending },
        "sessions_total": m.sessions_total.load(Ordering::Relaxed),
        "relay_bytes_total": m.relay_bytes.load(Ordering::Relaxed),
        "relay_mbps": m.relay_bps.load(Ordering::Relaxed) as f64 * 8.0 / 1e6,
        "connects_rejected": m.connects_rejected.load(Ordering::Relaxed),
        "auth_failures": m.auth_failures.load(Ordering::Relaxed),
        "auth_failures_last_hour": st.db.recent_failures(since).await,
        "errors": m.errors.load(Ordering::Relaxed),
        "cpu_percent": sys.cpu_percent,
        "memory_used": sys.mem_used,
        "memory_total": sys.mem_total,
        "process_memory": sys.process_mem,
        "client_versions": st.db.version_counts().await,
    }))
    .into_response()
}
