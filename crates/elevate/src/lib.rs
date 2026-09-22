//! Elevated (administrator) remote control.
//!
//! Windows does not let an ordinary desktop process type into administrator windows (UIPI) or see the
//! secure desktop that UAC prompts use. RemoteX therefore installs a small Windows service that can do
//! those two things on behalf of a session the owner of the PC explicitly approved:
//!
//! ```text
//!  RemoteX app (user) --local named pipe--> remotex-service (LocalSystem)
//!                                              | starts, only while a grant is active
//!                                              v
//!                                       remotex-service --agent (SYSTEM, inside the user's session)
//!                                              injects input / captures the secure desktop
//! ```
//!
//! This crate holds what the parties share: the message types, framing, size limits, the pixel
//! conversion used for secure-desktop pictures, and (on Windows) the pipe plumbing.
//! The service is deliberately **not** a generic privileged-command API: the only operations that exist
//! are the ones in [`UiToService`].

pub mod nv12;
#[cfg(windows)]
pub mod pipe;
pub mod proto;

pub use proto::*;
