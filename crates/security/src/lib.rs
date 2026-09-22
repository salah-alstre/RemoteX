//! Cryptographic building blocks. Nothing here invents primitives: SPAKE2 (PAKE), HKDF-SHA256,
//! HMAC-SHA256 and ChaCha20-Poly1305 are composed in the standard way.

pub mod channel;
pub mod dpapi;
pub mod ids;
pub mod limiter;
pub mod pake;
pub mod password;

pub use channel::{Opener, Sealer, SecureChannel};
pub use pake::{Keys, PakeError, PakeState, Role};
