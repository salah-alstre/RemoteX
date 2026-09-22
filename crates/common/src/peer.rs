//! Peer-to-peer protocol spoken end to end between viewer and host.
//! Handshake frames are plaintext (PAKE messages); everything after is AEAD-encrypted [`Msg`].

use serde::{Deserialize, Serialize};

pub const MAX_TEXT: usize = 1 << 20;
pub const MAX_CHUNK: usize = 256 * 1024;
pub const MAX_CURSOR_PIXELS: usize = 128 * 128 * 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthKind {
    Temporary,
    Permanent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Handshake {
    /// Viewer -> host.
    Hello {
        version: u16,
        viewer_id: String,
        viewer_name: String,
        auth: AuthKind,
        pake: Vec<u8>,
        requested: Permissions,
    },
    /// Host -> viewer.
    HelloReply { pake: Vec<u8> },
    /// Both directions: HMAC over the transcript proving knowledge of the PAKE key.
    Confirm { mac: Vec<u8> },
    /// Host -> viewer before the channel is keyed, e.g. session busy.
    Abort { reason: AbortReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AbortReason {
    Busy,
    Unsupported,
    NotAccepting,
    LockedOut,
    AuthFailed,
    UnattendedDisabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permissions {
    pub view: bool,
    pub mouse: bool,
    pub keyboard: bool,
    pub clipboard: bool,
    pub files: bool,
    /// Elevated (administrator) control through the installed RemoteX service. Never implied by the
    /// other permissions: the person at the host has to grant it explicitly for every session.
    pub elevated: bool,
}

impl Permissions {
    /// Every standard permission. Elevated control is deliberately not part of this set.
    pub const ALL: Self = Self {
        view: true,
        mouse: true,
        keyboard: true,
        clipboard: true,
        files: true,
        elevated: false,
    };
    pub const ALL_ELEVATED: Self = Self {
        elevated: true,
        ..Self::ALL
    };
    pub const NONE: Self = Self {
        view: false,
        mouse: false,
        keyboard: false,
        clipboard: false,
        files: false,
        elevated: false,
    };

    /// A grant can never exceed what was requested.
    pub fn intersect(self, other: Self) -> Self {
        Self {
            view: self.view && other.view,
            mouse: self.mouse && other.mouse,
            keyboard: self.keyboard && other.keyboard,
            clipboard: self.clipboard && other.clipboard,
            files: self.files && other.files,
            elevated: self.elevated && other.elevated,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    Declined,
    Timeout,
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Codec {
    H264,
    H265,
    Av1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preset {
    Auto,
    BestQuality,
    Balanced,
    LowLatency,
    LowBandwidth,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct QualitySettings {
    pub preset: Preset,
    /// 0 means automatic.
    pub fps: u8,
    /// Maximum output height, 0 means native/automatic.
    pub max_height: u16,
    /// Kilobits per second, 0 means automatic.
    pub bitrate_kbps: u32,
    pub codec: Option<Codec>,
    pub hardware: bool,
    /// When false the host keeps the configured quality even under congestion.
    pub adaptive: bool,
}

impl Default for QualitySettings {
    fn default() -> Self {
        Self {
            preset: Preset::Auto,
            fps: 0,
            max_height: 0,
            bitrate_kbps: 0,
            codec: None,
            hardware: true,
            adaptive: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub index: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoFrame {
    pub seq: u64,
    pub display: u32,
    pub codec: Codec,
    pub keyframe: bool,
    pub width: u32,
    pub height: u32,
    /// Host capture timestamp (ms since session start) for latency accounting.
    pub captured_ms: u64,
    /// Sequence number of the newest input event the host had applied when this frame was captured;
    /// the viewer uses it to measure motion-to-photon latency.
    pub input_seq: u32,
    /// Annex-B bitstream. Keyframes carry SPS/PPS (or VPS/SPS/PPS).
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorUpdate {
    pub x: i32,
    pub y: i32,
    pub visible: bool,
    /// Present only when the shape changed.
    pub shape: Option<CursorShape>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorShape {
    pub id: u32,
    pub width: u16,
    pub height: u16,
    pub hot_x: u16,
    pub hot_y: u16,
    /// Straight-alpha RGBA.
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    /// Position normalised to 0..=65535 across the selected display.
    MouseMove {
        x: u16,
        y: u16,
    },
    MouseButton {
        button: MouseButton,
        down: bool,
    },
    Wheel {
        dx: i16,
        dy: i16,
    },
    /// Physical key: set-1 scancode plus the E0 prefix flag. Layout independent.
    Key {
        scancode: u16,
        extended: bool,
        down: bool,
    },
    /// Unicode text for characters with no physical key (IME, Arabic composition, paste-as-typing).
    Text(String),
    ReleaseAll,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClipboardPayload {
    Text(String),
    /// PNG bytes.
    Image(Vec<u8>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileMsg {
    Offer {
        id: u64,
        name: String,
        size: u64,
    },
    Accept {
        id: u64,
        resume_from: u64,
    },
    Reject {
        id: u64,
        reason: String,
    },
    Chunk {
        id: u64,
        offset: u64,
        data: Vec<u8>,
    },
    Done {
        id: u64,
        sha256: [u8; 32],
    },
    Verified {
        id: u64,
        ok: bool,
    },
    Cancel {
        id: u64,
    },
    ListDir {
        path: String,
    },
    DirListing {
        path: String,
        entries: Vec<DirEntry>,
        error: Option<String>,
    },
    /// Ask the peer to send one of its files (path as returned by `DirListing`).
    Fetch {
        id: u64,
        path: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HostStats {
    pub fps: f32,
    pub bitrate_kbps: u32,
    pub capture_ms: f32,
    pub encode_ms: f32,
    pub dropped: u64,
    pub encoder: String,
    pub codec: Option<Codec>,
    pub width: u32,
    pub height: u32,
    /// Host-side input timing (microseconds, smoothed): how long an input event waited after arriving,
    /// and how long the operating system took to accept it.
    pub input_queue_us: u32,
    pub input_apply_us: u32,
    /// Mouse movements the host discarded because a newer position had already arrived.
    pub stale_moves_dropped: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Msg {
    Accepted {
        permissions: Permissions,
        displays: Vec<DisplayInfo>,
        active_display: u32,
        host_name: String,
    },
    Rejected(RejectReason),
    Video(VideoFrame),
    Cursor(CursorUpdate),
    PermissionsChanged(Permissions),
    DisplaysChanged {
        displays: Vec<DisplayInfo>,
        active: u32,
    },
    Stats(HostStats),

    SelectDisplay(u32),
    SetQuality(QualitySettings),
    RequestKeyframe,

    Clipboard(ClipboardPayload),
    File(FileMsg),
    Chat {
        id: u64,
        text: String,
        ts_ms: u64,
    },
    Ping {
        t: u64,
    },
    Pong {
        t: u64,
    },
    Bye,
    /// Input with a sequence number: mouse movement is droppable (newest wins), everything else is not.
    InputAt {
        seq: u32,
        ev: InputEvent,
    },
    /// Host -> viewer: the newest input event applied so far and how long it waited / took to apply.
    InputAck {
        seq: u32,
        queue_us: u32,
        apply_us: u32,
    },
}

impl Msg {
    /// Cheap structural validation applied to every decoded message before it is acted on.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Msg::Video(v) if v.data.len() > crate::wire::MAX_FRAME => Err("video too large"),
            Msg::InputAt {
                ev: InputEvent::Text(t),
                ..
            } if t.chars().count() > 4096 => Err("text input too long"),
            Msg::Clipboard(ClipboardPayload::Text(t)) if t.len() > MAX_TEXT => Err("clipboard too large"),
            Msg::Clipboard(ClipboardPayload::Image(i)) if i.len() > 16 * 1024 * 1024 => {
                Err("clipboard image too large")
            }
            Msg::Chat { text, .. } if text.chars().count() > 8192 => Err("chat too long"),
            Msg::File(FileMsg::Chunk { data, .. }) if data.len() > MAX_CHUNK => Err("chunk too large"),
            Msg::File(
                FileMsg::Offer { name, .. }
                | FileMsg::Fetch { path: name, .. }
                | FileMsg::ListDir { path: name },
            ) if name.len() > 1024 => Err("path too long"),
            Msg::Cursor(CursorUpdate { shape: Some(s), .. })
                if s.rgba.len() > MAX_CURSOR_PIXELS
                    || s.rgba.len() != s.width as usize * s.height as usize * 4 =>
            {
                Err("bad cursor shape")
            }
            Msg::DisplaysChanged { displays: d, .. } | Msg::Accepted { displays: d, .. } if d.len() > 32 => {
                Err("too many displays")
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire;

    #[test]
    fn roundtrip_and_reject_trailing_bytes() {
        let m = Msg::Chat {
            id: 1,
            text: "مرحبا".into(),
            ts_ms: 5,
        };
        let mut b = wire::encode(&m).unwrap();
        assert!(matches!(wire::decode::<Msg>(&b).unwrap(), Msg::Chat { .. }));
        b.push(0);
        assert!(wire::decode::<Msg>(&b).is_err());
    }

    #[test]
    fn garbage_never_panics() {
        for seed in 0u8..=255 {
            let buf: Vec<u8> = (0..64u8)
                .map(|i| i.wrapping_mul(seed).wrapping_add(seed))
                .collect();
            let _ = wire::decode::<Msg>(&buf);
            let _ = wire::decode::<Handshake>(&buf);
        }
    }

    #[test]
    fn validation_catches_oversize() {
        let big = Msg::File(FileMsg::Chunk {
            id: 1,
            offset: 0,
            data: vec![0; MAX_CHUNK + 1],
        });
        assert!(big.validate().is_err());
        let bad_cursor = Msg::Cursor(CursorUpdate {
            x: 0,
            y: 0,
            visible: true,
            shape: Some(CursorShape {
                id: 1,
                width: 2,
                height: 2,
                hot_x: 0,
                hot_y: 0,
                rgba: vec![0; 3],
            }),
        });
        assert!(bad_cursor.validate().is_err());
    }

    #[test]
    fn grant_never_exceeds_request() {
        let req = Permissions {
            mouse: false,
            ..Permissions::ALL
        };
        assert!(!Permissions::ALL.intersect(req).mouse);
    }
}
