//! Video encoding through Windows Media Foundation.
//!
//! Hardware encoders (NVIDIA NVENC, Intel Quick Sync, AMD AMF) are exposed by their drivers as
//! Media Foundation transforms, so one code path covers all three vendors. When no hardware
//! encoder accepts the requested configuration the Microsoft software encoder is used.

#![cfg(windows)]

mod annexb;
mod encoder;

pub use annexb::{nal_types, split_nals};
pub use encoder::{available_encoders, Encoder, EncoderConfig, EncoderInfo};

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("no encoder available for {0:?}")]
    NoEncoder(remotex_common::peer::Codec),
    #[error("codec not supported: {0:?}")]
    Unsupported(remotex_common::peer::Codec),
    #[error("encoder produced no output")]
    NoOutput,
    #[error("windows api error: {0}")]
    Windows(#[from] windows::core::Error),
}

pub struct EncodedFrame {
    /// Annex-B access unit.
    pub data: Vec<u8>,
    pub keyframe: bool,
}
