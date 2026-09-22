//! The one place that decides which RemoteX service the application talks to and which server
//! certificates it trusts. Everything else asks this module; nothing else knows a host or a pin.
//!
//! * Production values are embedded at build time from `config/production.json` (native, not editable
//!   by the user and never sent to or read from the web UI).
//! * Development builds (`dev-tools` feature only) may point at a local server through
//!   `REMOTEX_DEV_SERVER_OVERRIDE`. Release installers cannot be redirected.

use serde::Deserialize;

const PRODUCTION_JSON: &str = include_str!("../config/production.json");

#[derive(Debug, Deserialize)]
struct Raw {
    host: String,
    port: u16,
    path: String,
    pins: RawPins,
}

#[derive(Debug, Deserialize)]
struct RawPins {
    current: String,
    #[serde(default)]
    backup: String,
}

/// What the networking layer needs to connect securely.
#[derive(Debug, Clone, PartialEq)]
pub struct Infrastructure {
    pub url: String,
    /// Trusted leaf-certificate SHA-256 fingerprints (current + backup). Empty: normal web PKI.
    pub pins: Vec<[u8; 32]>,
    /// Plain `ws://` allowed. Only ever true in development builds.
    pub allow_insecure: bool,
}

fn parse_pins(raw: &RawPins) -> Vec<[u8; 32]> {
    [raw.current.as_str(), raw.backup.as_str()]
        .into_iter()
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            remotex_session::tls::parse_pin(p)
                .unwrap_or_else(|| panic!("invalid certificate pin in production.json"))
        })
        .collect()
}

fn from_json(json: &str) -> Infrastructure {
    let raw: Raw = serde_json::from_str(json).expect("config/production.json is valid");
    Infrastructure {
        url: format!("wss://{}:{}{}", raw.host, raw.port, raw.path),
        pins: parse_pins(&raw.pins),
        allow_insecure: false,
    }
}

/// The official production infrastructure.
pub fn production() -> Infrastructure {
    from_json(PRODUCTION_JSON)
}

/// Production, unless a development override is active (development builds only).
pub fn resolve() -> Infrastructure {
    #[allow(unused_mut)]
    let mut infra = production();
    #[cfg(feature = "dev-tools")]
    dev_override(&mut infra);
    infra
}

#[cfg(feature = "dev-tools")]
fn dev_override(infra: &mut Infrastructure) {
    let Some(url) = std::env::var("REMOTEX_DEV_SERVER_OVERRIDE")
        .ok()
        .filter(|u| !u.is_empty())
    else {
        return;
    };
    infra.allow_insecure = url.starts_with("ws://");
    infra.url = url;
    infra.pins = std::env::var("REMOTEX_DEV_CERT_PIN")
        .ok()
        .and_then(|p| remotex_session::tls::parse_pin(&p))
        .into_iter()
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_uses_tls_and_a_pin() {
        let p = production();
        assert!(p.url.starts_with("wss://"), "production must never use plain ws");
        assert!(!p.allow_insecure);
        assert!(!p.pins.is_empty(), "production must pin the server certificate");
    }

    #[test]
    fn production_matches_the_deployed_server() {
        let p = production();
        assert_eq!(p.url, "wss://45.88.9.191:8443/ws");
        let expected = remotex_session::tls::parse_pin(
            "3525ea50960ad2d5a39a526ead38abb74c5fd25eb6c738cb4f66a934df63cad0",
        )
        .unwrap();
        assert_eq!(p.pins, vec![expected]);
    }

    #[test]
    fn backup_pin_is_trusted_alongside_the_current_one() {
        let json = r#"{"host":"api.example.com","port":443,"path":"/ws",
            "pins":{"current":"aa00000000000000000000000000000000000000000000000000000000000000",
                    "backup":"bb00000000000000000000000000000000000000000000000000000000000000"}}"#;
        let i = from_json(json);
        assert_eq!(i.url, "wss://api.example.com:443/ws");
        assert_eq!(i.pins.len(), 2);
        assert_ne!(i.pins[0], i.pins[1]);
    }

    #[test]
    fn no_pins_means_normal_certificate_validation() {
        let json = r#"{"host":"api.example.com","port":443,"path":"/ws","pins":{"current":"","backup":""}}"#;
        assert!(from_json(json).pins.is_empty());
    }

    #[test]
    #[should_panic(expected = "invalid certificate pin")]
    fn malformed_pins_fail_loudly_instead_of_weakening_validation() {
        from_json(r#"{"host":"h","port":1,"path":"/","pins":{"current":"not-a-pin","backup":""}}"#);
    }
}
