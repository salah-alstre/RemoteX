//! Clipboard synchronisation logic independent of the operating system.
//!
//! Loop prevention: content written because of a remote update is remembered by hash, so the
//! change notification it causes locally is not echoed back to the peer.

use remotex_common::peer::ClipboardPayload;
use sha2::{Digest, Sha256};

pub trait ClipboardBackend {
    /// Monotonic counter that changes whenever the clipboard content changes.
    fn sequence(&self) -> u32;
    fn read(&mut self) -> Option<ClipboardPayload>;
    fn write(&mut self, payload: &ClipboardPayload) -> Result<(), String>;
}

impl<T: ClipboardBackend + ?Sized> ClipboardBackend for Box<T> {
    fn sequence(&self) -> u32 {
        (**self).sequence()
    }
    fn read(&mut self) -> Option<ClipboardPayload> {
        (**self).read()
    }
    fn write(&mut self, payload: &ClipboardPayload) -> Result<(), String> {
        (**self).write(payload)
    }
}

pub struct ClipboardSync<B: ClipboardBackend> {
    backend: B,
    last_seq: u32,
    suppressed: Option<[u8; 32]>,
    last_sent: Option<[u8; 32]>,
    max_bytes: usize,
}

fn digest(p: &ClipboardPayload) -> [u8; 32] {
    let mut h = Sha256::new();
    match p {
        ClipboardPayload::Text(t) => {
            h.update([0u8]);
            h.update(t.as_bytes());
        }
        ClipboardPayload::Image(i) => {
            h.update([1u8]);
            h.update(i);
        }
    }
    h.finalize().into()
}

fn size(p: &ClipboardPayload) -> usize {
    match p {
        ClipboardPayload::Text(t) => t.len(),
        ClipboardPayload::Image(i) => i.len(),
    }
}

impl<B: ClipboardBackend> ClipboardSync<B> {
    pub fn new(backend: B, max_bytes: usize) -> Self {
        let last_seq = backend.sequence();
        Self {
            backend,
            last_seq,
            suppressed: None,
            last_sent: None,
            max_bytes,
        }
    }

    /// Returns new local clipboard content that should be sent to the peer.
    pub fn poll_local(&mut self) -> Option<ClipboardPayload> {
        let seq = self.backend.sequence();
        if seq == self.last_seq {
            return None;
        }
        self.last_seq = seq;
        let content = self.backend.read()?;
        if size(&content) == 0 || size(&content) > self.max_bytes {
            return None;
        }
        let d = digest(&content);
        if self.suppressed == Some(d) {
            self.suppressed = None;
            return None;
        }
        if self.last_sent == Some(d) {
            return None;
        }
        self.last_sent = Some(d);
        Some(content)
    }

    /// Applies content received from the peer.
    pub fn apply_remote(&mut self, payload: &ClipboardPayload) -> Result<(), String> {
        if size(payload) > self.max_bytes {
            return Err("clipboard content too large".into());
        }
        let d = digest(payload);
        self.suppressed = Some(d);
        self.last_sent = Some(d);
        self.backend.write(payload)?;
        self.last_seq = self.backend.sequence();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Mock {
        seq: u32,
        content: Option<ClipboardPayload>,
    }
    impl ClipboardBackend for Mock {
        fn sequence(&self) -> u32 {
            self.seq
        }
        fn read(&mut self) -> Option<ClipboardPayload> {
            self.content.clone()
        }
        fn write(&mut self, p: &ClipboardPayload) -> Result<(), String> {
            self.seq += 1;
            self.content = Some(p.clone());
            Ok(())
        }
    }

    fn text(s: &str) -> ClipboardPayload {
        ClipboardPayload::Text(s.into())
    }

    #[test]
    fn local_change_is_sent_once() {
        let mut sync = ClipboardSync::new(Mock::default(), 1000);
        assert!(sync.poll_local().is_none());
        sync.backend.seq += 1;
        sync.backend.content = Some(text("hello"));
        assert!(matches!(sync.poll_local(), Some(ClipboardPayload::Text(t)) if t == "hello"));
        assert!(sync.poll_local().is_none());
        // Same text copied again is not resent.
        sync.backend.seq += 1;
        assert!(sync.poll_local().is_none());
    }

    #[test]
    fn remote_update_does_not_echo() {
        let mut sync = ClipboardSync::new(Mock::default(), 1000);
        sync.apply_remote(&text("from peer")).unwrap();
        assert!(
            sync.poll_local().is_none(),
            "applying a remote value must not bounce it back"
        );
        // A later genuine local copy still flows.
        sync.backend.seq += 1;
        sync.backend.content = Some(text("local"));
        assert!(sync.poll_local().is_some());
    }

    #[test]
    fn echo_is_suppressed_even_if_sequence_bumps_twice() {
        let mut sync = ClipboardSync::new(Mock::default(), 1000);
        sync.apply_remote(&text("x")).unwrap();
        sync.backend.seq += 1; // a second change notification for the same content
        assert!(sync.poll_local().is_none());
    }

    #[test]
    fn oversize_content_is_dropped_both_ways() {
        let mut sync = ClipboardSync::new(Mock::default(), 4);
        assert!(sync.apply_remote(&text("too long")).is_err());
        sync.backend.seq += 1;
        sync.backend.content = Some(text("too long"));
        assert!(sync.poll_local().is_none());
    }
}
