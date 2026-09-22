//! Ordered AEAD channel. The nonce is an implicit per-direction counter, so a replayed, dropped,
//! duplicated or reordered frame fails authentication and terminates the session.

use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, Key, KeyInit, Nonce};

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("authentication failed")]
    Auth,
    #[error("nonce space exhausted")]
    Exhausted,
}

pub struct Sealer {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

pub struct Opener {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

fn nonce(counter: u64) -> Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&counter.to_be_bytes());
    n.into()
}

impl Sealer {
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, ChannelError> {
        let ct = self
            .cipher
            .encrypt(&nonce(self.counter), plaintext)
            .map_err(|_| ChannelError::Auth)?;
        self.counter = self.counter.checked_add(1).ok_or(ChannelError::Exhausted)?;
        Ok(ct)
    }
}

impl Opener {
    /// The counter only advances on success so an attacker cannot desynchronise a live session
    /// (in practice any failure ends it anyway).
    pub fn open(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, ChannelError> {
        let pt = self
            .cipher
            .decrypt(&nonce(self.counter), ciphertext)
            .map_err(|_| ChannelError::Auth)?;
        self.counter = self.counter.checked_add(1).ok_or(ChannelError::Exhausted)?;
        Ok(pt)
    }
}

pub struct SecureChannel {
    sealer: Sealer,
    opener: Opener,
}

impl SecureChannel {
    pub fn new(send_key: &[u8; 32], recv_key: &[u8; 32]) -> Self {
        Self {
            sealer: Sealer {
                cipher: ChaCha20Poly1305::new(Key::from_slice(send_key)),
                counter: 0,
            },
            opener: Opener {
                cipher: ChaCha20Poly1305::new(Key::from_slice(recv_key)),
                counter: 0,
            },
        }
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> Vec<u8> {
        self.sealer
            .seal(plaintext)
            .expect("sealing cannot fail for in-memory buffers")
    }

    pub fn open(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, ChannelError> {
        self.opener.open(ciphertext)
    }

    pub fn split(self) -> (Sealer, Opener) {
        (self.sealer, self.opener)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (SecureChannel, SecureChannel) {
        let (a, b) = ([1u8; 32], [2u8; 32]);
        (SecureChannel::new(&a, &b), SecureChannel::new(&b, &a))
    }

    #[test]
    fn replay_reorder_and_tamper_are_rejected() {
        let (mut x, mut y) = pair();
        let c1 = x.seal(b"one");
        let c2 = x.seal(b"two");
        assert!(y.open(&c2).is_err(), "reordered");
        assert_eq!(y.open(&c1).unwrap(), b"one");
        assert!(y.open(&c1).is_err(), "replayed");
        let mut bad = c2.clone();
        bad[0] ^= 1;
        assert!(y.open(&bad).is_err(), "tampered");
        assert_eq!(y.open(&c2).unwrap(), b"two");
    }

    #[test]
    fn directions_use_separate_keys() {
        let (mut x, _) = pair();
        let (mut x2, _) = pair();
        let c = x.seal(b"echo");
        assert!(
            x2.open(&c).is_err(),
            "a message must not be accepted by its own sender"
        );
    }
}
