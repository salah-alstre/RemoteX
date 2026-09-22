//! Client <-> server messages. The server sees only this metadata plus opaque relayed ciphertext.

use crate::SessionId;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 2;
pub const MAX_NAME_LEN: usize = 64;
pub const MAX_CANDIDATES: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientToServer {
    /// First run: ask the server to allocate a new device id and secret.
    Register {
        client_version: String,
    },
    Auth {
        device_id: String,
        secret: Vec<u8>,
        client_version: String,
        device_name: String,
        /// `ip:port` addresses of the host's direct listener on local interfaces.
        candidates: Vec<String>,
        incoming_enabled: bool,
    },
    Connect {
        target_id: String,
    },
    Relay {
        session: SessionId,
        data: Vec<u8>,
    },
    /// `auth_failed` lets the host report a bad password so the server can throttle the source.
    EndSession {
        session: SessionId,
        auth_failed: bool,
    },
    /// Viewer reports how the session was established so the server can account for it.
    SessionMode {
        session: SessionId,
        direct: bool,
    },
    QueryPresence {
        ids: Vec<String>,
    },
    SetIncomingEnabled(bool),
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerToClient {
    Registered {
        device_id: String,
        secret: Vec<u8>,
    },
    AuthOk {
        public_ip: String,
    },
    AuthFailed {
        reason: String,
    },
    /// Sent to the host.
    Incoming {
        session: SessionId,
        from_id: String,
        from_name: String,
        same_network: bool,
    },
    /// Sent to the viewer.
    SessionReady {
        session: SessionId,
        host_name: String,
        candidates: Vec<String>,
        same_network: bool,
    },
    ConnectFailed {
        target_id: String,
        reason: ConnectFailure,
    },
    Relay {
        session: SessionId,
        data: Vec<u8>,
    },
    SessionEnded {
        session: SessionId,
    },
    Presence(Vec<(String, bool)>),
    Pong,
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectFailure {
    Offline,
    NotAccepting,
    RateLimited,
    Banned,
    Busy,
    InvalidId,
}
