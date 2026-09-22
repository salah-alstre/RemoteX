//! TLS client configuration. Certificate verification is never disabled: either the normal web PKI
//! applies (no pins configured, e.g. after a move to a domain with a CA-issued certificate), or the leaf
//! certificate must match one of the configured SHA-256 pins (current + backup, for rotation).

use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{ring as provider, verify_tls12_signature, verify_tls13_signature, CryptoProvider},
    pki_types::{CertificateDer, ServerName, UnixTime},
    ClientConfig, DigitallySignedStruct, Error, RootCertStore, SignatureScheme,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Debug)]
struct PinVerifier {
    pins: Vec<[u8; 32]>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let got: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if self.pins.contains(&got) {
            tracing::info!(target: "security", event = "tls.pin.valid", "tls.pin.valid");
            Ok(ServerCertVerified::assertion())
        } else {
            // A short prefix of the (public) fingerprint is enough to spot a TLS-inspecting antivirus or proxy.
            let presented: String = got.iter().take(4).map(|x| format!("{x:02x}")).collect();
            tracing::warn!(target: "security", event = "tls.pin.mismatch", %presented, "tls.pin.mismatch: the presented certificate is not the pinned one");
            Err(Error::General(
                "server certificate does not match a trusted fingerprint".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub fn parse_pin(hex_pin: &str) -> Option<[u8; 32]> {
    let cleaned: String = hex_pin.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    hex::decode(cleaned).ok()?.try_into().ok()
}

/// `pins` empty: normal web PKI. Otherwise the server certificate must match one of the pins.
pub fn client_config(pins: &[[u8; 32]]) -> Arc<ClientConfig> {
    let provider = Arc::new(provider::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions");
    let config = match pins.is_empty() {
        false => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinVerifier {
                pins: pins.to_vec(),
                provider,
            }))
            .with_no_client_auth(),
        true => {
            let mut roots = RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            builder.with_root_certificates(roots).with_no_client_auth()
        }
    };
    Arc::new(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_parsing_accepts_common_formats() {
        let hex = "aa".repeat(32);
        assert!(parse_pin(&hex).is_some());
        let colon = vec!["AA"; 32].join(":");
        assert_eq!(parse_pin(&colon), parse_pin(&hex));
        assert!(parse_pin("abcd").is_none());
        assert!(parse_pin("").is_none());
    }
}
