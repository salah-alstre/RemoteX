use remotex_common::branding::Branding;
use remotex_session::{
    store::{DeviceBook, SecretFile, Settings},
    Engine,
};
use std::{
    path::PathBuf,
    sync::{atomic::AtomicUsize, Mutex, MutexGuard},
};
use tauri::ipc::{Channel, InvokeResponseBody};

pub struct AppState {
    pub engine: Engine,
    pub data_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub settings: Mutex<Settings>,
    pub book: Mutex<DeviceBook>,
    pub secret: SecretFile,
    pub brand: Branding,
    pub video: Mutex<Option<Channel<InvokeResponseBody>>>,
    /// Frames sent to the webview that it has not yet acknowledged; used to shed load safely.
    pub inflight: AtomicUsize,
    /// Video frames the shell discarded because the webview was still busy with older ones.
    pub stale_video: std::sync::atomic::AtomicU64,
    pub tray: Mutex<Option<crate::tray::TrayHandles>>,
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl AppState {
    pub fn settings_path(&self) -> PathBuf {
        self.data_dir.join("settings.json")
    }

    pub fn book_path(&self) -> PathBuf {
        self.data_dir.join("devices.json")
    }

    pub fn save_settings(&self) {
        if let Err(e) = lock(&self.settings).save(&self.settings_path()) {
            tracing::error!(error = %e, "could not save settings");
        }
    }

    pub fn save_book(&self) {
        if let Err(e) = lock(&self.book).save(&self.book_path()) {
            tracing::error!(error = %e, "could not save address book");
        }
    }
}
