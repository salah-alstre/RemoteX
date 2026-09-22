//! Password-authenticated key exchange between viewer and host.
//!
//! A short temporary password would be trivially brute-forced offline if the relay could record a
//! challenge/response. SPAKE2 makes every guess cost one live interaction with the host, and the
//! resulting key is bound to the session and both identities.

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use remotex_common::SessionId;
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::channel::SecureChannel;

#[derive(Debug, thiserror::Error)]
pub enum PakeError {
    #[error("invalid key exchange message")]
    BadMessage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Viewer,
    Host,
}

pub struct PakeState {
    role: Role,
    spake: Spake2<Ed25519Group>,
    my_msg: Vec<u8>,
    session: SessionId,
}

impl PakeState {
    pub fn start(role: Role, password: &[u8], session: &SessionId, viewer_id: &str, host_id: &str) -> Self {
        let pw = Password::new(password);
        // Identities bind the exchange to this session and to both parties.
        let id_v = Identity::new(&[session.0.as_slice(), b"|viewer|", viewer_id.as_bytes()].concat());
        let id_h = Identity::new(&[session.0.as_slice(), b"|host|", host_id.as_bytes()].concat());
        let (spake, my_msg) = match role {
            Role::Viewer => Spake2::<Ed25519Group>::start_a(&pw, &id_v, &id_h),
            Role::Host => Spake2::<Ed25519Group>::start_b(&pw, &id_v, &id_h),
        };
        Self {
            role,
            spake,
            my_msg,
            session: *session,
        }
    }

    pub fn message(&self) -> &[u8] {
        &self.my_msg
    }

    /// Completes the exchange. The result only agrees between both sides if the passwords matched;
    /// that is proven by the [`Keys::confirm_mac`] round.
    pub fn finish(self, peer_msg: &[u8]) -> Result<Keys, PakeError> {
        let key = Zeroizing::new(self.spake.finish(peer_msg).map_err(|_| PakeError::BadMessage)?);
        let (viewer_msg, host_msg) = match self.role {
            Role::Viewer => (self.my_msg.as_slice(), peer_msg),
            Role::Host => (peer_msg, self.my_msg.as_slice()),
        };
        let transcript = Sha256::new()
            .chain_update(b"remotex-transcript-v1")
            .chain_update(self.session.0)
            .chain_update((viewer_msg.len() as u32).to_be_bytes())
            .chain_update(viewer_msg)
            .chain_update((host_msg.len() as u32).to_be_bytes())
            .chain_update(host_msg)
            .finalize();

        let hk = Hkdf::<Sha256>::new(Some(&transcript), &key);
        let mut okm = Zeroizing::new([0u8; 128]);
        hk.expand(b"remotex-session-keys-v1", okm.as_mut())
            .expect("128 bytes is a valid HKDF length");
        let take = |i: usize| -> [u8; 32] { okm[i * 32..(i + 1) * 32].try_into().expect("32 bytes") };
        Ok(Keys {
            role: self.role,
            v2h: take(0),
            h2v: take(1),
            confirm_v: take(2),
            confirm_h: take(3),
            transcript: transcript.into(),
        })
    }
}

pub struct Keys {
    role: Role,
    v2h: [u8; 32],
    h2v: [u8; 32],
    confirm_v: [u8; 32],
    confirm_h: [u8; 32],
    transcript: [u8; 32],
}

type HmacSha256 = Hmac<Sha256>;

impl Keys {
    fn mac(&self, key: &[u8; 32], label: &[u8]) -> Vec<u8> {
        let mut m = <HmacSha256 as Mac>::new_from_slice(key).expect("hmac accepts any key length");
        m.update(label);
        m.update(&self.transcript);
        m.finalize().into_bytes().to_vec()
    }

    /// MAC this side sends to prove it derived the same key.
    pub fn my_confirm(&self) -> Vec<u8> {
        match self.role {
            Role::Viewer => self.mac(&self.confirm_v, b"viewer-confirm"),
            Role::Host => self.mac(&self.confirm_h, b"host-confirm"),
        }
    }

    /// Constant-time check of the peer's confirmation MAC.
    pub fn verify_peer_confirm(&self, mac: &[u8]) -> bool {
        let expected = match self.role {
            Role::Viewer => self.mac(&self.confirm_h, b"host-confirm"),
            Role::Host => self.mac(&self.confirm_v, b"viewer-confirm"),
        };
        expected.ct_eq(mac).into()
    }

    pub fn into_channel(self) -> SecureChannel {
        match self.role {
            Role::Viewer => SecureChannel::new(&self.v2h, &self.h2v),
            Role::Host => SecureChannel::new(&self.h2v, &self.v2h),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::generate_session_id;

    fn run(pw_viewer: &[u8], pw_host: &[u8]) -> (Keys, Keys) {
        let s = generate_session_id();
        let v = PakeState::start(Role::Viewer, pw_viewer, &s, "111111111", "222222222");
        let h = PakeState::start(Role::Host, pw_host, &s, "111111111", "222222222");
        let (vm, hm) = (v.message().to_vec(), h.message().to_vec());
        (v.finish(&hm).unwrap(), h.finish(&vm).unwrap())
    }

    #[test]
    fn matching_passwords_confirm_and_talk() {
        let (v, h) = run(b"K7F92Q", b"K7F92Q");
        assert!(h.verify_peer_confirm(&v.my_confirm()));
        assert!(v.verify_peer_confirm(&h.my_confirm()));
        let (mut vc, mut hc) = (v.into_channel(), h.into_channel());
        let ct = vc.seal(b"hello");
        assert_eq!(hc.open(&ct).unwrap(), b"hello");
        let ct = hc.seal(b"world");
        assert_eq!(vc.open(&ct).unwrap(), b"world");
    }

    #[test]
    fn wrong_password_fails_confirmation() {
        let (v, h) = run(b"K7F92Q", b"WRONG1");
        assert!(!h.verify_peer_confirm(&v.my_confirm()));
        assert!(!v.verify_peer_confirm(&h.my_confirm()));
    }

    #[test]
    fn confirmations_are_not_reflectable() {
        let (v, h) = run(b"K7F92Q", b"K7F92Q");
        // A viewer's own MAC must not satisfy the viewer's check of the host.
        assert!(!v.verify_peer_confirm(&v.my_confirm()));
        assert!(!h.verify_peer_confirm(&h.my_confirm()));
    }

    #[test]
    fn garbage_peer_message_is_rejected() {
        let s = generate_session_id();
        let v = PakeState::start(Role::Viewer, b"x", &s, "1", "2");
        assert!(v.finish(&[1, 2, 3]).is_err());
    }
}
