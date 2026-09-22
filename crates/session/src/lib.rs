//! RemoteX session engine: signaling, authenticated encrypted sessions, host/viewer logic.

pub mod adapt;
pub mod bulk;
#[cfg(windows)]
pub mod elevation;
pub mod engine;
mod engine_host;
mod engine_viewer;
pub mod env;
pub mod handshake;
pub mod host;
pub mod input_thread;
pub mod link;
#[cfg(windows)]
pub mod platform;
pub mod secure;
pub mod signaling;
pub mod store;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod tls;
pub mod transfer;
pub mod types;
pub mod video;
pub mod viewer;

pub use engine::Engine;
pub use engine_viewer::{backoff_secs, MAX_RECONNECT_ATTEMPTS};
