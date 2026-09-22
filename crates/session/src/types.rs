//! Types shared between the engine and its front end. Everything here serialises to JSON for the UI,
//! except `Event::Video`, which is delivered over a binary channel.

use crate::link::LinkKind;
use remotex_common::peer::{Codec, CursorUpdate, DirEntry, DisplayInfo, HostStats, Permissions, VideoFrame};
use serde::Serialize;
use std::{path::PathBuf, time::Duration};
use tokio::sync::mpsc;

pub type EventTx = mpsc::UnboundedSender<Event>;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ServerState {
    Starting,
    Connecting,
    Online,
    Reconnecting {
        attempt: u32,
    },
    /// The computer itself appears to have no internet access.
    NoInternet,
    /// The internet works but the RemoteX service cannot be reached.
    ServiceUnavailable,
    /// The service refused this device (for example a temporary block).
    Rejected,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingRequest {
    pub request_id: u64,
    pub viewer_id: String,
    pub viewer_name: String,
    pub requested: Permissions,
    pub same_network: bool,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ViewerState {
    Connecting,
    Authenticating,
    WaitingForApproval,
    Connected,
    Reconnecting { attempt: u32, next_in_secs: u32 },
    Failed { reason: FailReason },
    Closed,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FailReason {
    Offline,
    NotAccepting,
    RateLimited,
    Banned,
    Busy,
    InvalidId,
    ServerUnavailable,
    WrongPassword,
    LockedOut,
    Declined,
    Timeout,
    UnattendedDisabled,
    Unsupported,
    ConnectionLost,
    Protocol,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkInfo {
    pub kind: LinkKind,
    pub peer: String,
    /// The transport under the link: LAN sessions are TCP direct, relay sessions ride the server's TLS WebSocket.
    pub protocol: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerReady {
    pub host_name: String,
    pub displays: Vec<DisplayInfo>,
    pub permissions: Permissions,
    pub active_display: u32,
    pub link: LinkInfo,
}

/// Viewer-side telemetry computed from real measurements (RTT from ping/pong, bytes from the socket).
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ViewerStats {
    pub rtt_ms: f32,
    pub jitter_ms: f32,
    pub recv_kbps: u32,
    pub frames_per_sec: f32,
    /// Share of frames the host skipped because the network could not keep up. The transport is a
    /// reliable stream, so congestion shows up as delay and skipped frames rather than lost packets.
    pub dropped_percent: f32,
    pub quality: NetQuality,
    pub host: HostStatsView,
    pub latency: LatencyView,
}

/// Measured responsiveness of the control loop. All values are real measurements (monotonic clocks);
/// zero means "not measured yet".
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LatencyView {
    /// Send -> host applied -> acknowledgement received.
    pub input_rtt_ms: f32,
    /// Time input waited in this machine's send queue.
    pub input_queue_ms: f32,
    /// Time input waited on the host before being applied, and the host OS time to accept it.
    pub host_queue_ms: f32,
    pub host_apply_ms: f32,
    /// Send -> a video frame that reflects the input has arrived here (before decode and paint).
    pub motion_to_frame_ms: f32,
    pub stale_moves_dropped: u64,
    pub stale_video_dropped: u64,
    pub file_rate_kbps: u32,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum NetQuality {
    Excellent,
    Good,
    Fair,
    #[default]
    Poor,
}

impl NetQuality {
    /// Rates the link from measured round-trip time, jitter and dropped-frame share.
    pub fn rate(rtt_ms: f32, jitter_ms: f32, dropped_percent: f32) -> Self {
        if rtt_ms < 35.0 && jitter_ms < 12.0 && dropped_percent < 1.0 {
            Self::Excellent
        } else if rtt_ms < 90.0 && jitter_ms < 30.0 && dropped_percent < 4.0 {
            Self::Good
        } else if rtt_ms < 220.0 && jitter_ms < 80.0 && dropped_percent < 12.0 {
            Self::Fair
        } else {
            Self::Poor
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HostStatsView {
    pub fps: f32,
    pub bitrate_kbps: u32,
    pub capture_ms: f32,
    pub encode_ms: f32,
    pub dropped: u64,
    pub encoder: String,
    pub codec: Option<Codec>,
    pub width: u32,
    pub height: u32,
    pub input_queue_us: u32,
    pub input_apply_us: u32,
    pub stale_moves_dropped: u64,
}

impl From<HostStats> for HostStatsView {
    fn from(s: HostStats) -> Self {
        Self {
            fps: s.fps,
            bitrate_kbps: s.bitrate_kbps,
            capture_ms: s.capture_ms,
            encode_ms: s.encode_ms,
            dropped: s.dropped,
            encoder: s.encoder,
            codec: s.codec,
            width: s.width,
            height: s.height,
            input_queue_us: s.input_queue_us,
            input_apply_us: s.input_apply_us,
            stale_moves_dropped: s.stale_moves_dropped,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferInfo {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub transferred: u64,
    pub upload: bool,
    pub status: TransferStatus,
    pub speed_bps: u64,
    pub eta_secs: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TransferStatus {
    Pending,
    Active,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EndReason {
    PeerClosed,
    LocalClosed,
    ConnectionLost,
    Kicked,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Event {
    Server(ServerState),
    Identity {
        id: String,
    },
    TempPassword {
        password: String,
        expires_in_secs: Option<u64>,
    },
    Incoming(IncomingRequest),
    IncomingCancelled {
        request_id: u64,
    },
    HostStarted {
        viewer_id: String,
        viewer_name: String,
        permissions: Permissions,
        link: LinkInfo,
    },
    HostPermissions(Permissions),
    HostEnded {
        reason: EndReason,
    },
    HostNotice {
        notice: HostNotice,
    },
    Viewer {
        session: u64,
        state: ViewerState,
    },
    ViewerReady {
        session: u64,
        info: ViewerReady,
    },
    ViewerDisplays {
        session: u64,
        displays: Vec<DisplayInfo>,
        active: u32,
    },
    ViewerPermissions {
        session: u64,
        permissions: Permissions,
    },
    ViewerStats {
        session: u64,
        stats: ViewerStats,
    },
    #[serde(skip)]
    Video {
        session: u64,
        frame: VideoFrame,
    },
    Cursor {
        session: u64,
        update: CursorUpdateView,
    },
    Clipboard {
        session: u64,
        text: Option<String>,
        image: bool,
    },
    Chat {
        session: u64,
        id: u64,
        text: String,
        ts_ms: u64,
        mine: bool,
    },
    Transfer {
        session: u64,
        info: TransferInfo,
    },
    FileRequest {
        session: u64,
        id: u64,
        name: String,
        size: u64,
    },
    DirListing {
        session: u64,
        path: String,
        entries: Vec<DirEntry>,
        error: Option<String>,
    },
    Presence {
        statuses: Vec<(String, bool)>,
    },
    Security {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "notice", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum HostNotice {
    /// The viewer was granted elevated control but the service needed for it is not available.
    ElevationUnavailable,
    LockedOut,
    PasswordRejected {
        viewer_id: String,
    },
    FileReceived {
        name: String,
    },
    Declined {
        viewer_id: String,
    },
}

/// Cursor update in a JSON-friendly form (shape pixels base64 encoded to keep the JSON compact).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorUpdateView {
    pub x: i32,
    pub y: i32,
    pub visible: bool,
    pub shape: Option<CursorShapeView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorShapeView {
    pub id: u32,
    pub width: u16,
    pub height: u16,
    pub hot_x: u16,
    pub hot_y: u16,
    pub rgba_base64: String,
}

impl From<CursorUpdate> for CursorUpdateView {
    fn from(u: CursorUpdate) -> Self {
        Self {
            x: u.x,
            y: u.y,
            visible: u.visible,
            shape: u.shape.map(|s| CursorShapeView {
                id: s.id,
                width: s.width,
                height: s.height,
                hot_x: s.hot_x,
                hot_y: s.hot_y,
                rgba_base64: base64(&s.rgba),
            }),
        }
    }
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Static configuration for an engine instance.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub server_url: String,
    /// Trusted certificate fingerprints (current + backup). Empty means normal web PKI validation.
    pub cert_pins: Vec<[u8; 32]>,
    /// Accept plain `ws://` (development builds only).
    pub allow_insecure: bool,
    pub client_version: String,
    pub device_name: String,
    pub download_dir: PathBuf,
    pub ask_before_receiving: bool,
    pub max_file_size: u64,
    pub max_transfers: usize,
    pub temp_password_ttl: Option<Duration>,
    pub rotate_after_session: bool,
    pub direct_enabled: bool,
    /// When false a session that cannot go direct fails instead of using the server relay.
    pub relay_fallback: bool,
    pub incoming_enabled: bool,
    pub clipboard_images: bool,
    pub approval_timeout: Duration,
}

impl EngineConfig {
    pub fn new(server_url: impl Into<String>, device_name: impl Into<String>, download_dir: PathBuf) -> Self {
        Self {
            server_url: server_url.into(),
            cert_pins: Vec::new(),
            allow_insecure: false,
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            device_name: device_name.into(),
            download_dir,
            ask_before_receiving: true,
            max_file_size: remotex_files::DEFAULT_MAX_FILE,
            max_transfers: 3,
            temp_password_ttl: None,
            rotate_after_session: true,
            direct_enabled: true,
            relay_fallback: true,
            incoming_enabled: true,
            clipboard_images: true,
            approval_timeout: Duration::from_secs(60),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_quality_thresholds() {
        assert_eq!(NetQuality::rate(10.0, 2.0, 0.0), NetQuality::Excellent);
        assert_eq!(NetQuality::rate(60.0, 5.0, 0.0), NetQuality::Good);
        assert_eq!(NetQuality::rate(10.0, 2.0, 6.0), NetQuality::Fair);
        assert_eq!(NetQuality::rate(400.0, 2.0, 0.0), NetQuality::Poor);
        assert_eq!(NetQuality::rate(30.0, 50.0, 0.0), NetQuality::Fair);
    }

    #[test]
    fn base64_matches_reference_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
