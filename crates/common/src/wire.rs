//! Bounded (de)serialisation helpers. Every frame that crosses a trust boundary goes through here.

use serde::{de::DeserializeOwned, Serialize};

/// Hard cap for a single frame, including encryption overhead.
pub const MAX_FRAME: usize = 8 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("frame too large ({0} bytes)")]
    TooLarge(usize),
    #[error("malformed message")]
    Malformed,
}

pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, WireError> {
    let bytes = postcard::to_stdvec(msg).map_err(|_| WireError::Malformed)?;
    if bytes.len() > MAX_FRAME {
        return Err(WireError::TooLarge(bytes.len()));
    }
    Ok(bytes)
}

/// Decodes and requires the whole buffer to be consumed, rejecting trailing garbage.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, WireError> {
    if bytes.len() > MAX_FRAME {
        return Err(WireError::TooLarge(bytes.len()));
    }
    let (value, rest) = postcard::take_from_bytes::<T>(bytes).map_err(|_| WireError::Malformed)?;
    if !rest.is_empty() {
        return Err(WireError::Malformed);
    }
    Ok(value)
}
