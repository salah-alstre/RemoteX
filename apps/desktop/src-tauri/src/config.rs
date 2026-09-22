//! Turns persisted settings plus branding into an engine configuration.

use crate::infra;
use remotex_session::{store::Settings, types::EngineConfig};
use std::{path::Path, time::Duration};

pub fn computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "My PC".into())
}

pub fn default_download_dir() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|p| Path::new(&p).join("Downloads").join("RemoteX"))
        .unwrap_or_else(|| std::path::PathBuf::from("downloads"))
}

pub fn engine_config(s: &Settings) -> EngineConfig {
    let infra = infra::resolve();
    let name = if s.device_name.trim().is_empty() {
        computer_name()
    } else {
        s.device_name.trim().chars().take(64).collect()
    };
    let dl = if s.download_dir.trim().is_empty() {
        default_download_dir()
    } else {
        s.download_dir.clone().into()
    };
    let mut cfg = EngineConfig::new(infra.url, name, dl);
    cfg.cert_pins = infra.pins;
    cfg.allow_insecure = infra.allow_insecure;
    cfg.ask_before_receiving = s.ask_before_receiving;
    cfg.max_transfers = s.max_transfers.clamp(1, 8) as usize;
    cfg.temp_password_ttl = (s.temp_password_expiry_mins > 0)
        .then(|| Duration::from_secs(s.temp_password_expiry_mins as u64 * 60));
    cfg.rotate_after_session = s.rotate_after_session;
    cfg.direct_enabled = s.prefer_direct;
    cfg.relay_fallback = s.relay_fallback;
    cfg.incoming_enabled = s.incoming_enabled;
    cfg.clipboard_images = s.clipboard_images;
    cfg.client_version = env!("CARGO_PKG_VERSION").to_string();
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_map_to_engine_config() {
        let s = Settings {
            temp_password_expiry_mins: 15,
            ask_before_receiving: false,
            max_transfers: 5,
            prefer_direct: false,
            relay_fallback: false,
            device_name: "  Office PC ".into(),
            ..Default::default()
        };
        let c = engine_config(&s);
        assert_eq!(c.temp_password_ttl, Some(Duration::from_secs(900)));
        assert!(!c.ask_before_receiving && !c.direct_enabled && !c.relay_fallback);
        assert_eq!((c.max_transfers, c.device_name.as_str()), (5, "Office PC"));
    }

    #[test]
    fn users_cannot_change_the_service_or_weaken_validation() {
        // No persisted setting can influence the endpoint: the config always equals production.
        let c = engine_config(&Settings::default());
        let prod = infra::production();
        assert_eq!(c.server_url, prod.url);
        assert_eq!(c.cert_pins, prod.pins);
        assert!(!c.cert_pins.is_empty());
        assert!(
            !c.allow_insecure,
            "insecure transport is never enabled in a normal build"
        );
    }
}
