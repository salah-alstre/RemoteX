//! Local persistence: settings, trusted devices, address book, and the DPAPI-protected identity.
//! Files are written atomically (temp file + rename) and a corrupt file never blocks startup.

use crate::signaling::{CredentialStore, Credentials};
use remotex_common::peer::{Codec, Permissions, Preset, QualitySettings};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

/// Loads JSON, falling back to defaults (and keeping the bad file as `.bak`) if it cannot be parsed.
fn load_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    match fs::read(path) {
        // Editors such as Notepad prepend a UTF-8 byte order mark that JSON parsers reject.
        Ok(bytes) => serde_json::from_slice(bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&bytes))
            .unwrap_or_else(|_| {
                let _ = fs::rename(path, path.with_extension("bak"));
                T::default()
            }),
        Err(_) => T::default(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub language: String,
    pub theme: String,
    pub start_with_windows: bool,
    pub minimize_to_tray: bool,
    pub launch_minimized: bool,
    pub close_behavior: String,
    pub check_updates: bool,
    pub first_run_done: bool,
    pub device_name: String,

    pub preset: String,
    pub codec: String,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub resolution: String,
    pub hardware_acceleration: bool,
    pub prefer_direct: bool,
    pub relay_fallback: bool,
    pub adaptive_quality: bool,
    pub low_bandwidth: bool,
    pub performance_mode: bool,
    pub show_remote_cursor: bool,
    /// "auto", "on" or "off": draw the cursor at the local pointer immediately and reconcile with the host.
    pub cursor_prediction: String,
    pub scale_mode: String,

    pub incoming_enabled: bool,
    pub temp_password_expiry_mins: u32,
    pub rotate_after_session: bool,
    pub default_permissions: Permissions,
    pub clipboard_sync: bool,
    pub clipboard_images: bool,

    pub unattended_enabled: bool,
    pub unattended_any_device: bool,
    /// Unattended sessions may use elevated (administrator) control. Off by default and only meaningful
    /// when unattended access itself is on.
    pub unattended_elevated: bool,

    pub download_dir: String,
    pub ask_before_receiving: bool,
    pub max_transfers: u32,
    pub open_dir_after_transfer: bool,

    pub ui_scale: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "system".into(),
            theme: "system".into(),
            start_with_windows: false,
            minimize_to_tray: true,
            launch_minimized: false,
            close_behavior: "tray".into(),
            check_updates: true,
            first_run_done: false,
            device_name: String::new(),
            preset: "auto".into(),
            codec: "auto".into(),
            fps: 0,
            bitrate_kbps: 0,
            resolution: "auto".into(),
            hardware_acceleration: true,
            prefer_direct: true,
            relay_fallback: true,
            adaptive_quality: true,
            low_bandwidth: false,
            performance_mode: false,
            show_remote_cursor: true,
            cursor_prediction: "auto".into(),
            scale_mode: "fit".into(),
            incoming_enabled: true,
            temp_password_expiry_mins: 0,
            rotate_after_session: true,
            default_permissions: Permissions::ALL,
            clipboard_sync: true,
            clipboard_images: true,
            unattended_enabled: false,
            unattended_any_device: false,
            unattended_elevated: false,
            download_dir: String::new(),
            ask_before_receiving: true,
            max_transfers: 3,
            open_dir_after_transfer: false,
            ui_scale: 100,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        load_json(path)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        write_atomic(
            path,
            &serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?,
        )
    }

    /// Streaming quality requested from hosts, derived from the user's connection settings.
    pub fn quality(&self) -> QualitySettings {
        let preset = match self.preset.as_str() {
            "best" => Preset::BestQuality,
            "balanced" => Preset::Balanced,
            "lowLatency" => Preset::LowLatency,
            "lowBandwidth" => Preset::LowBandwidth,
            _ if self.low_bandwidth => Preset::LowBandwidth,
            _ if self.performance_mode => Preset::LowLatency,
            _ => Preset::Auto,
        };
        let max_height = match self.resolution.as_str() {
            "auto" => 0,
            // Native: never scale below the display, so adaptation cannot shrink the picture.
            "native" => 4320,
            other => other.parse::<u16>().map(|h| h.clamp(180, 4320)).unwrap_or(0),
        };
        let codec = match self.codec.as_str() {
            "h264" => Some(Codec::H264),
            "h265" => Some(Codec::H265),
            _ => None,
        };
        QualitySettings {
            preset,
            fps: self.fps as u8,
            max_height,
            bitrate_kbps: self.bitrate_kbps,
            codec,
            hardware: self.hardware_acceleration,
            adaptive: self.adaptive_quality,
        }
    }

    /// Clamps values that came from an untrusted or hand-edited file into supported ranges.
    pub fn sanitized(mut self) -> Self {
        self.max_transfers = self.max_transfers.clamp(1, 8);
        self.ui_scale = self.ui_scale.clamp(80, 160);
        if !matches!(self.fps, 0 | 15 | 30 | 60) {
            self.fps = 0;
        }
        self.bitrate_kbps = if self.bitrate_kbps == 0 {
            0
        } else {
            self.bitrate_kbps.clamp(200, 100_000)
        };
        if !matches!(self.language.as_str(), "system" | "en" | "ar") {
            self.language = "system".into();
        }
        if !matches!(self.theme.as_str(), "system" | "light" | "dark") {
            self.theme = "system".into();
        }
        if !matches!(self.cursor_prediction.as_str(), "auto" | "on" | "off") {
            self.cursor_prediction = "auto".into();
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrustedDevice {
    pub id: String,
    pub name: String,
    pub added_ms: u64,
    pub last_connection_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AddressEntry {
    pub id: String,
    pub name: String,
    pub favorite: bool,
    /// "my" for the user's own machines, "other" for everything else.
    pub group: String,
    pub added_ms: u64,
    pub last_connected_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Recent {
    pub id: String,
    pub last_ms: u64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct DeviceBook {
    pub trusted: Vec<TrustedDevice>,
    pub entries: Vec<AddressEntry>,
    pub recents: Vec<Recent>,
}

const MAX_RECENTS: usize = 20;
const MAX_ENTRIES: usize = 500;

impl DeviceBook {
    pub fn load(path: &Path) -> Self {
        load_json(path)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        write_atomic(
            path,
            &serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?,
        )
    }

    pub fn trust(&mut self, id: &str, name: &str, now_ms: u64) {
        if !self.trusted.iter().any(|t| t.id == id) {
            self.trusted.push(TrustedDevice {
                id: id.into(),
                name: name.into(),
                added_ms: now_ms,
                last_connection_ms: Some(now_ms),
            });
        }
    }

    pub fn revoke(&mut self, id: &str) -> bool {
        let before = self.trusted.len();
        self.trusted.retain(|t| t.id != id);
        before != self.trusted.len()
    }

    pub fn revoke_all(&mut self) {
        self.trusted.clear();
    }

    pub fn rename_trusted(&mut self, id: &str, name: &str) {
        if let Some(t) = self.trusted.iter_mut().find(|t| t.id == id) {
            t.name = name.chars().take(64).collect();
        }
    }

    pub fn touch_trusted(&mut self, id: &str, now_ms: u64) {
        if let Some(t) = self.trusted.iter_mut().find(|t| t.id == id) {
            t.last_connection_ms = Some(now_ms);
        }
    }

    pub fn record_connection(&mut self, id: &str, now_ms: u64) {
        self.recents.retain(|r| r.id != id);
        self.recents.insert(
            0,
            Recent {
                id: id.into(),
                last_ms: now_ms,
            },
        );
        self.recents.truncate(MAX_RECENTS);
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            e.last_connected_ms = Some(now_ms);
        }
    }

    pub fn upsert_entry(
        &mut self,
        id: &str,
        name: &str,
        favorite: bool,
        group: &str,
        now_ms: u64,
    ) -> Result<(), &'static str> {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            e.name = name.chars().take(64).collect();
            e.favorite = favorite;
            e.group = group.into();
            return Ok(());
        }
        if self.entries.len() >= MAX_ENTRIES {
            return Err("address book is full");
        }
        self.entries.push(AddressEntry {
            id: id.into(),
            name: name.chars().take(64).collect(),
            favorite,
            group: if group == "my" {
                "my".into()
            } else {
                "other".into()
            },
            added_ms: now_ms,
            last_connected_ms: None,
        });
        Ok(())
    }

    pub fn remove_entry(&mut self, id: &str) {
        self.entries.retain(|e| e.id != id);
    }
}

/// Device identity protected with DPAPI: readable only by the same Windows user on the same machine.
pub struct DpapiCredentialStore {
    path: PathBuf,
    cache: Mutex<Option<Credentials>>,
}

#[derive(Serialize, Deserialize)]
struct StoredCreds {
    id: String,
    secret: Vec<u8>,
}

impl DpapiCredentialStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            cache: Mutex::new(None),
        }
    }
}

impl CredentialStore for DpapiCredentialStore {
    fn load(&self) -> Option<Credentials> {
        if let Some(c) = self.cache.lock().ok()?.clone() {
            return Some(c);
        }
        let blob = match fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                tracing::warn!(target: "security", event = "identity.unreadable", error = %e, "identity.unreadable");
                return None;
            }
        };
        let plain = match remotex_security::dpapi::unprotect(&blob) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(target: "security", event = "identity.unreadable", error = %e, "identity.unreadable: cannot be decrypted on this account, a new identity will be created");
                return None;
            }
        };
        let Ok(s) = serde_json::from_slice::<StoredCreds>(&plain) else {
            tracing::warn!(target: "security", event = "identity.unreadable", "identity.unreadable: corrupt, a new identity will be created");
            return None;
        };
        tracing::info!(target: "server_connection", event = "identity.loaded", "identity.loaded");
        let c = Credentials {
            id: s.id,
            secret: s.secret,
        };
        *self.cache.lock().ok()? = Some(c.clone());
        Some(c)
    }

    fn save(&self, creds: &Credentials) {
        let json = serde_json::to_vec(&StoredCreds {
            id: creds.id.clone(),
            secret: creds.secret.clone(),
        })
        .unwrap_or_default();
        match remotex_security::dpapi::protect(&json) {
            Ok(blob) => match write_atomic(&self.path, &blob) {
                Ok(()) => {
                    tracing::info!(target: "server_connection", event = "device_id.persisted", "device_id.persisted")
                }
                Err(e) => {
                    tracing::error!(target: "security", event = "device_id.persist_failed", error = %e, "could not persist device identity")
                }
            },
            Err(e) => tracing::error!(error = %e, "could not protect device identity"),
        }
        if let Ok(mut c) = self.cache.lock() {
            *c = Some(creds.clone());
        }
    }

    fn clear(&self) {
        let _ = fs::remove_file(&self.path);
        if let Ok(mut c) = self.cache.lock() {
            *c = None;
        }
    }
}

/// DPAPI-protected single secret (the unattended-access password).
pub struct SecretFile {
    path: PathBuf,
}

impl SecretFile {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn store(&self, secret: &str) -> Result<(), String> {
        let blob = remotex_security::dpapi::protect(secret.as_bytes()).map_err(|e| e.to_string())?;
        write_atomic(&self.path, &blob).map_err(|e| e.to_string())
    }

    pub fn load(&self) -> Option<String> {
        let blob = fs::read(&self.path).ok()?;
        String::from_utf8(remotex_security::dpapi::unprotect(&blob).ok()?).ok()
    }

    pub fn clear(&self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn settings_roundtrip_and_defaults_for_missing_fields() {
        let d = tempdir().unwrap();
        let p = d.path().join("settings.json");
        let mut s = Settings {
            language: "ar".into(),
            theme: "dark".into(),
            ..Default::default()
        };
        s.fps = 60;
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p), s);
        // Older file without newer fields still loads.
        fs::write(&p, br#"{"language":"en"}"#).unwrap();
        let loaded = Settings::load(&p);
        assert_eq!(loaded.language, "en");
        assert!(loaded.rotate_after_session);
    }

    #[test]
    fn settings_written_with_a_byte_order_mark_still_load() {
        let d = tempdir().unwrap();
        let p = d.path().join("settings.json");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(br#"{"language":"ar","theme":"dark"}"#);
        fs::write(&p, bytes).unwrap();
        let s = Settings::load(&p);
        assert_eq!((s.language.as_str(), s.theme.as_str()), ("ar", "dark"));
        assert!(!d.path().join("settings.bak").exists());
    }

    #[test]
    fn corrupt_settings_fall_back_and_keep_backup() {
        let d = tempdir().unwrap();
        let p = d.path().join("settings.json");
        fs::write(&p, b"{ not json").unwrap();
        assert_eq!(Settings::load(&p), Settings::default());
        assert!(d.path().join("settings.bak").exists());
    }

    #[test]
    fn sanitizing_clamps_hostile_values() {
        let s = Settings {
            max_transfers: 9999,
            ui_scale: 5,
            fps: 999,
            bitrate_kbps: 5,
            language: "xx".into(),
            theme: "neon".into(),
            ..Default::default()
        }
        .sanitized();
        assert_eq!(
            (s.max_transfers, s.ui_scale, s.fps, s.bitrate_kbps),
            (8, 80, 0, 200)
        );
        assert_eq!((s.language.as_str(), s.theme.as_str()), ("system", "system"));
    }

    #[test]
    fn quality_mapping_follows_settings() {
        let mut s = Settings::default();
        assert_eq!(s.quality(), QualitySettings::default());
        s.performance_mode = true;
        assert_eq!(s.quality().preset, Preset::LowLatency);
        s.low_bandwidth = true;
        assert_eq!(s.quality().preset, Preset::LowBandwidth);
        s.preset = "best".into();
        assert_eq!(s.quality().preset, Preset::BestQuality, "explicit preset wins");
        s.resolution = "720".into();
        s.fps = 30;
        s.codec = "h265".into();
        s.adaptive_quality = false;
        let q = s.quality();
        assert_eq!(
            (q.max_height, q.fps, q.codec, q.adaptive),
            (720, 30, Some(Codec::H265), false)
        );
        s.resolution = "native".into();
        assert_eq!(s.quality().max_height, 4320);
    }

    #[test]
    fn trusted_devices_lifecycle() {
        let mut b = DeviceBook::default();
        b.trust("111222333", "Office PC", 10);
        b.trust("111222333", "Duplicate", 20);
        assert_eq!(b.trusted.len(), 1);
        b.rename_trusted("111222333", "Work laptop");
        assert_eq!(b.trusted[0].name, "Work laptop");
        b.touch_trusted("111222333", 99);
        assert_eq!(b.trusted[0].last_connection_ms, Some(99));
        assert!(b.revoke("111222333"));
        assert!(!b.revoke("111222333"));
        b.trust("1", "a", 0);
        b.trust("2", "b", 0);
        b.revoke_all();
        assert!(b.trusted.is_empty());
    }

    #[test]
    fn recents_are_deduplicated_ordered_and_capped() {
        let mut b = DeviceBook::default();
        for i in 0..30u64 {
            b.record_connection(&format!("10000000{i:02}"), i);
        }
        assert_eq!(b.recents.len(), MAX_RECENTS);
        b.record_connection("1000000015", 100);
        assert_eq!(b.recents[0].id, "1000000015");
        assert_eq!(b.recents.iter().filter(|r| r.id == "1000000015").count(), 1);
    }

    #[test]
    fn address_book_entries_and_persistence() {
        let d = tempdir().unwrap();
        let p = d.path().join("devices.json");
        let mut b = DeviceBook::default();
        b.upsert_entry("583294814", "Office PC", true, "my", 1).unwrap();
        b.upsert_entry("583294814", "Office", false, "my", 2).unwrap();
        assert_eq!(b.entries.len(), 1);
        b.record_connection("583294814", 50);
        assert_eq!(b.entries[0].last_connected_ms, Some(50));
        b.save(&p).unwrap();
        assert_eq!(DeviceBook::load(&p), b);
    }

    #[cfg(windows)]
    #[test]
    fn identity_is_encrypted_at_rest() {
        let d = tempdir().unwrap();
        let store = DpapiCredentialStore::new(d.path().join("identity.bin"));
        assert!(store.load().is_none());
        store.save(&Credentials {
            id: "583294814".into(),
            secret: vec![7; 32],
        });
        let raw = fs::read(d.path().join("identity.bin")).unwrap();
        assert!(!raw.windows(9).any(|w| w == b"583294814"));
        let fresh = DpapiCredentialStore::new(d.path().join("identity.bin"));
        let c = fresh.load().unwrap();
        assert_eq!((c.id.as_str(), c.secret.len()), ("583294814", 32));
        fresh.clear();
        assert!(fresh.load().is_none());

        let secret = SecretFile::new(d.path().join("unattended.bin"));
        secret.store("s3cret-pass").unwrap();
        assert_eq!(secret.load().as_deref(), Some("s3cret-pass"));
        assert!(!fs::read(d.path().join("unattended.bin"))
            .unwrap()
            .windows(6)
            .any(|w| w == b"s3cret"));
    }
}
