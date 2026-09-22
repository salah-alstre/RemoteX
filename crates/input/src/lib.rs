//! Remote input injection and clipboard synchronisation.

pub mod clipboard;
#[cfg(windows)]
pub mod injector;
#[cfg(windows)]
pub mod system_clipboard;

pub use clipboard::{ClipboardBackend, ClipboardSync};
