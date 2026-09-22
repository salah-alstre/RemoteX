//! Tauri commands: the thin, validated surface between the webview and the engine.

use crate::{
    config,
    state::{lock, AppState},
};
use remotex_common::{
    peer::{AuthKind, InputEvent, MouseButton, Permissions},
    signaling::ConnectFailure,
    DeviceId,
};
use remotex_session::{
    host::HostCmd,
    store::{AddressEntry, DeviceBook, Settings, TrustedDevice},
    viewer::ViewerCmd,
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use tauri::{
    ipc::{Channel, InvokeResponseBody},
    AppHandle, State,
};
use tauri_plugin_opener::OpenerExt;

type S<'a> = State<'a, Arc<AppState>>;
type CmdResult<T = ()> = Result<T, String>;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub id: Option<String>,
    pub password: String,
    pub settings: Settings,
    pub book: DeviceBook,
    pub version: &'static str,
    pub build: &'static str,
    pub brand: BrandView,
    pub encoders: Vec<EncoderView>,
    pub server_online: bool,
    pub computer_name: String,
    pub downloads_dir: String,
    pub data_dir: String,
    pub unattended_password_set: bool,
    pub os_language: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrandView {
    pub name: String,
    pub company: String,
    pub website: String,
    pub github: String,
    pub support_email: String,
}

#[derive(Serialize)]
pub struct EncoderView {
    pub codec: String,
    pub name: String,
    pub hardware: bool,
}

#[tauri::command]
pub fn get_bootstrap(state: S) -> Bootstrap {
    // The live values (id, password, service state) are read last: the slow parts below (encoder probing,
    // DPAPI) must not make them stale, or a fast registration could be overwritten by an older snapshot.
    let settings = lock(&state.settings).clone();
    let cfg = config::engine_config(&settings);
    let mut boot = Bootstrap {
        id: state.engine.my_id().map(|d| d.display()),
        password: state.engine.temp_password(),
        book: lock(&state.book).clone(),
        version: env!("CARGO_PKG_VERSION"),
        build: option_env!("REMOTEX_BUILD").unwrap_or("dev"),
        brand: BrandView {
            name: state.brand.name.clone(),
            company: state.brand.company.clone(),
            website: state.brand.website.clone(),
            github: state.brand.github.clone(),
            support_email: state.brand.support_email.clone(),
        },
        encoders: state
            .engine
            .available_encoders()
            .into_iter()
            .map(|(c, name, hardware)| EncoderView {
                codec: format!("{c:?}"),
                name,
                hardware,
            })
            .collect(),
        server_online: state.engine.server_online(),
        computer_name: config::computer_name(),
        downloads_dir: cfg.download_dir.display().to_string(),
        data_dir: state.data_dir.display().to_string(),
        unattended_password_set: state.secret.load().is_some(),
        os_language: sys_locale::get_locale().unwrap_or_default(),
        settings,
    };
    boot.id = state.engine.my_id().map(|d| d.display());
    boot.password = state.engine.temp_password();
    boot.server_online = state.engine.server_online();
    boot
}

/// Applies a full settings object: validates, persists and pushes the effective values into the engine.
#[tauri::command]
pub fn settings_save(app: AppHandle, state: S, settings: Settings) -> CmdResult<Settings> {
    let clean = settings.sanitized();
    let previous = std::mem::replace(&mut *lock(&state.settings), clean.clone());
    state.save_settings();
    state.engine.update_config(config::engine_config(&clean));
    state.engine.set_incoming_enabled(clean.incoming_enabled);
    if let Some(t) = lock(&state.tray).as_ref() {
        t.set_incoming(clean.incoming_enabled);
    }
    apply_unattended(&state);
    if clean.start_with_windows != previous.start_with_windows {
        use tauri_plugin_autostart::ManagerExt;
        let mgr = app.autolaunch();
        let r = if clean.start_with_windows {
            mgr.enable()
        } else {
            mgr.disable()
        };
        r.map_err(|e| e.to_string())?;
    }
    Ok(clean)
}

fn apply_unattended(state: &AppState) {
    let s = lock(&state.settings).clone();
    let authorized = lock(&state.book).trusted.iter().map(|t| t.id.clone()).collect();
    state.engine.set_unattended(
        s.unattended_enabled,
        state.secret.load(),
        authorized,
        s.unattended_any_device,
    );
    state.engine.set_unattended_elevated(s.unattended_elevated);
}

/// Whether elevated control can be offered on this PC (the service is installed and running).
#[tauri::command]
pub async fn elevation_available(state: S<'_>) -> Result<bool, String> {
    let engine = state.engine.clone();
    tokio::task::spawn_blocking(move || engine.elevation_available())
        .await
        .map_err(|e| e.to_string())
}

pub fn restore_unattended(state: &AppState) {
    apply_unattended(state);
}

#[tauri::command]
pub fn unattended_set_password(state: S, password: String) -> CmdResult {
    if password.chars().count() < 8 {
        return Err("tooShort".into());
    }
    state.secret.store(&password)?;
    apply_unattended(&state);
    Ok(())
}

#[tauri::command]
pub fn unattended_clear_password(state: S) {
    state.secret.clear();
    apply_unattended(&state);
}

#[tauri::command]
pub fn regenerate_password(state: S) -> String {
    state.engine.regenerate_password()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectArgs {
    pub target: String,
    pub password: String,
    pub unattended: bool,
    /// The viewer asks for Full / Elevated control (the owner of the PC still decides).
    #[serde(default)]
    pub elevated: bool,
}

fn connect_error(e: ConnectFailure) -> String {
    format!("{e:?}")
}

#[tauri::command]
pub fn connect(state: S, args: ConnectArgs) -> CmdResult<u64> {
    let settings = lock(&state.settings).clone();
    let kind = if args.unattended {
        AuthKind::Permanent
    } else {
        AuthKind::Temporary
    };
    let requested = Permissions {
        elevated: args.elevated,
        ..settings.default_permissions
    };
    let session = state
        .engine
        .connect(&args.target, &args.password, kind, requested)
        .map_err(connect_error)?;
    if let Some(id) = DeviceId::parse(&args.target) {
        lock(&state.book).record_connection(id.as_str(), now_ms());
        lock(&state.book).touch_trusted(id.as_str(), now_ms());
        state.save_book();
    }
    Ok(session)
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UiInput {
    MouseMove {
        x: u16,
        y: u16,
    },
    MouseButton {
        button: u8,
        down: bool,
    },
    Wheel {
        dx: i16,
        dy: i16,
    },
    Key {
        scancode: u16,
        extended: bool,
        down: bool,
    },
    Text {
        text: String,
    },
    ReleaseAll,
}

impl From<UiInput> for InputEvent {
    fn from(i: UiInput) -> Self {
        match i {
            UiInput::MouseMove { x, y } => InputEvent::MouseMove { x, y },
            UiInput::MouseButton { button, down } => InputEvent::MouseButton {
                button: match button {
                    0 => MouseButton::Left,
                    1 => MouseButton::Middle,
                    2 => MouseButton::Right,
                    3 => MouseButton::Back,
                    _ => MouseButton::Forward,
                },
                down,
            },
            UiInput::Wheel { dx, dy } => InputEvent::Wheel { dx, dy },
            UiInput::Key {
                scancode,
                extended,
                down,
            } => InputEvent::Key {
                scancode,
                extended,
                down,
            },
            UiInput::Text { text } => InputEvent::Text(text.chars().take(4096).collect()),
            UiInput::ReleaseAll => InputEvent::ReleaseAll,
        }
    }
}

/// Decodes the compact binary input packet sent by the viewer page:
/// `session u32 LE | kind u8 | payload` where the payload is
/// 0 move `x u16, y u16` | 1 button `button u8, down u8` | 2 wheel `dx i16, dy i16` |
/// 3 key `scancode u16, extended u8, down u8` | 4 text `utf-8` | 5 release-all.
pub fn decode_input(b: &[u8]) -> Option<(u64, InputEvent)> {
    let session = u32::from_le_bytes(b.get(0..4)?.try_into().ok()?) as u64;
    let p = b.get(5..)?;
    let u16_at = |i: usize| p.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
    let ev = match *b.get(4)? {
        0 => InputEvent::MouseMove {
            x: u16_at(0)?,
            y: u16_at(2)?,
        },
        1 => UiInput::MouseButton {
            button: *p.first()?,
            down: *p.get(1)? != 0,
        }
        .into(),
        2 => InputEvent::Wheel {
            dx: u16_at(0)? as i16,
            dy: u16_at(2)? as i16,
        },
        3 => InputEvent::Key {
            scancode: u16_at(0)?,
            extended: *p.get(2)? != 0,
            down: *p.get(3)? != 0,
        },
        4 => InputEvent::Text(std::str::from_utf8(p).ok()?.to_string()),
        5 => InputEvent::ReleaseAll,
        _ => return None,
    };
    Some((session, ev))
}

/// High-rate input path: a raw binary body (no JSON), and `async` so it never queues behind work on the
/// application's main thread.
#[tauri::command]
pub async fn viewer_input_bin(state: S<'_>, request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("expected a binary body".into());
    };
    let (session, ev) = decode_input(bytes).ok_or("malformed input packet")?;
    state.engine.viewer_command(session, ViewerCmd::Input(ev));
    Ok(())
}

#[tauri::command]
pub fn viewer_input(state: S, session: u64, event: UiInput) {
    state
        .engine
        .viewer_command(session, ViewerCmd::Input(event.into()));
}

#[tauri::command]
pub fn viewer_select_display(state: S, session: u64, index: u32) {
    state
        .engine
        .viewer_command(session, ViewerCmd::SelectDisplay(index));
}

/// `overrides` is a partial settings object (preset, fps, resolution, bitrateKbps, codec, ...).
#[tauri::command]
pub fn viewer_set_quality(state: S, session: u64, overrides: serde_json::Value) -> CmdResult {
    let mut base = serde_json::to_value(lock(&state.settings).clone()).map_err(|e| e.to_string())?;
    if let (Some(b), Some(o)) = (base.as_object_mut(), overrides.as_object()) {
        for (k, v) in o {
            b.insert(k.clone(), v.clone());
        }
    }
    let merged: Settings = serde_json::from_value(base).map_err(|e| e.to_string())?;
    state
        .engine
        .viewer_command(session, ViewerCmd::SetQuality(merged.sanitized().quality()));
    Ok(())
}

#[tauri::command]
pub fn viewer_keyframe(state: S, session: u64) {
    state.engine.viewer_command(session, ViewerCmd::RequestKeyframe);
}

#[tauri::command]
pub fn viewer_clipboard_sync(state: S, session: u64, on: bool) {
    state
        .engine
        .viewer_command(session, ViewerCmd::SetClipboardSync(on));
}

#[tauri::command]
pub fn viewer_chat(state: S, session: u64, text: String) {
    let text: String = text.trim().chars().take(4000).collect();
    if !text.is_empty() {
        state.engine.viewer_command(session, ViewerCmd::Chat(text));
    }
}

#[tauri::command]
pub fn viewer_send_files(state: S, session: u64, paths: Vec<String>) {
    state.engine.viewer_command(
        session,
        ViewerCmd::SendFiles(paths.into_iter().map(PathBuf::from).collect()),
    );
}

#[tauri::command]
pub fn viewer_download(state: S, session: u64, path: String) {
    state.engine.viewer_command(session, ViewerCmd::Download(path));
}

#[tauri::command]
pub fn viewer_list_dir(state: S, session: u64, path: String) {
    state.engine.viewer_command(session, ViewerCmd::ListDir(path));
}

#[tauri::command]
pub fn viewer_file(state: S, session: u64, action: String, id: u64) -> CmdResult {
    let cmd = match action.as_str() {
        "pause" => ViewerCmd::FilePause(id),
        "resume" => ViewerCmd::FileResume(id),
        "cancel" => ViewerCmd::FileCancel(id),
        "retry" => ViewerCmd::FileRetry(id),
        _ => return Err("unknown action".into()),
    };
    state.engine.viewer_command(session, cmd);
    Ok(())
}

#[tauri::command]
pub fn viewer_disconnect(state: S, session: u64) {
    state.engine.viewer_command(session, ViewerCmd::Disconnect);
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingDecision {
    pub request_id: u64,
    pub permissions: Option<Permissions>,
    pub trust: bool,
    pub viewer_id: String,
    pub viewer_name: String,
}

#[tauri::command]
pub fn respond_incoming(state: S, decision: IncomingDecision) {
    if decision.permissions.is_some() && decision.trust {
        lock(&state.book).trust(&decision.viewer_id, &decision.viewer_name, now_ms());
        state.save_book();
        apply_unattended(&state);
    }
    state
        .engine
        .respond_incoming(decision.request_id, decision.permissions);
}

#[tauri::command]
pub fn host_set_permissions(state: S, permissions: Permissions) {
    state.engine.host_command(HostCmd::SetPermissions(permissions));
}

#[tauri::command]
pub fn host_chat(state: S, text: String) {
    let text: String = text.trim().chars().take(4000).collect();
    if !text.is_empty() {
        state.engine.host_command(HostCmd::Chat(text));
    }
}

#[tauri::command]
pub fn host_kick(state: S) {
    state.engine.host_command(HostCmd::Kick);
}

#[tauri::command]
pub fn host_approve_file(state: S, id: u64, accept: bool) {
    state.engine.host_command(HostCmd::ApproveFile { id, accept });
}

// ---- address book & trusted devices ----------------------------------------------------------

fn valid_id(id: &str) -> CmdResult<String> {
    DeviceId::parse(id)
        .map(|d| d.as_str().to_string())
        .ok_or_else(|| "invalidId".to_string())
}

#[tauri::command]
pub fn book_trust(state: S, id: String, name: String) -> CmdResult<Vec<TrustedDevice>> {
    let id = valid_id(&id)?;
    lock(&state.book).trust(
        &id,
        name.trim().chars().take(64).collect::<String>().as_str(),
        now_ms(),
    );
    state.save_book();
    apply_unattended(&state);
    Ok(lock(&state.book).trusted.clone())
}

#[tauri::command]
pub fn book_revoke(state: S, id: String) -> Vec<TrustedDevice> {
    lock(&state.book).revoke(&id);
    state.save_book();
    apply_unattended(&state);
    lock(&state.book).trusted.clone()
}

#[tauri::command]
pub fn book_revoke_all(state: S) -> Vec<TrustedDevice> {
    lock(&state.book).revoke_all();
    state.save_book();
    apply_unattended(&state);
    Vec::new()
}

#[tauri::command]
pub fn book_rename_trusted(state: S, id: String, name: String) -> Vec<TrustedDevice> {
    lock(&state.book).rename_trusted(&id, name.trim());
    state.save_book();
    lock(&state.book).trusted.clone()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryArgs {
    pub id: String,
    pub name: String,
    pub favorite: bool,
    pub group: String,
}

#[tauri::command]
pub fn book_upsert(state: S, entry: EntryArgs) -> CmdResult<Vec<AddressEntry>> {
    let id = valid_id(&entry.id)?;
    lock(&state.book)
        .upsert_entry(&id, entry.name.trim(), entry.favorite, &entry.group, now_ms())
        .map_err(String::from)?;
    state.save_book();
    Ok(lock(&state.book).entries.clone())
}

#[tauri::command]
pub fn book_remove(state: S, id: String) -> Vec<AddressEntry> {
    lock(&state.book).remove_entry(&id);
    state.save_book();
    lock(&state.book).entries.clone()
}

#[tauri::command]
pub async fn presence_query(state: S<'_>, ids: Vec<String>) -> CmdResult {
    let ids: Vec<String> = ids.into_iter().take(64).collect();
    state.engine.query_presence(ids).await;
    Ok(())
}

// ---- video channel ---------------------------------------------------------------------------

#[tauri::command]
pub fn video_subscribe(state: S, channel: Channel<InvokeResponseBody>) {
    *lock(&state.video) = Some(channel);
    state.inflight.store(0, std::sync::atomic::Ordering::SeqCst);
}

#[tauri::command]
pub fn video_ack(state: S, count: usize) {
    let _ = state.inflight.fetch_update(
        std::sync::atomic::Ordering::SeqCst,
        std::sync::atomic::Ordering::SeqCst,
        |v| Some(v.saturating_sub(count)),
    );
}

// ---- misc -------------------------------------------------------------------------------------

#[tauri::command]
pub fn open_folder(app: AppHandle, state: S, kind: String) -> CmdResult {
    let path = match kind.as_str() {
        "logs" => state.logs_dir.clone(),
        "downloads" => config::engine_config(&lock(&state.settings)).download_dir,
        "data" => state.data_dir.clone(),
        _ => return Err("unknown folder".into()),
    };
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Recent security-relevant log lines for the Security settings page.
#[tauri::command]
pub fn read_security_log(state: S, limit: usize) -> Vec<String> {
    let mut files: Vec<_> = std::fs::read_dir(&state.logs_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("security.log"))
        .collect();
    files.sort_by_key(|e| e.file_name());
    let Some(last) = files.last() else {
        return Vec::new();
    };
    let text = std::fs::read_to_string(last.path()).unwrap_or_default();
    text.lines()
        .rev()
        .take(limit.min(500))
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(kind: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = 7u32.to_le_bytes().to_vec();
        v.push(kind);
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn binary_input_packets_decode() {
        let (s, e) = decode_input(&packet(0, &[0x34, 0x12, 0xff, 0xff])).unwrap();
        assert_eq!((s, e), (7, InputEvent::MouseMove { x: 0x1234, y: 0xffff }));
        let (_, e) = decode_input(&packet(1, &[2, 1])).unwrap();
        assert_eq!(
            e,
            InputEvent::MouseButton {
                button: MouseButton::Right,
                down: true
            }
        );
        let (_, e) = decode_input(&packet(2, &[0xfe, 0xff, 0x0a, 0x00])).unwrap();
        assert_eq!(e, InputEvent::Wheel { dx: -2, dy: 10 });
        let (_, e) = decode_input(&packet(3, &[0x1e, 0x00, 0, 1])).unwrap();
        assert_eq!(
            e,
            InputEvent::Key {
                scancode: 0x1e,
                extended: false,
                down: true
            }
        );
        let (_, e) = decode_input(&packet(4, "مرحبا".as_bytes())).unwrap();
        assert_eq!(e, InputEvent::Text("مرحبا".into()));
        assert_eq!(decode_input(&packet(5, &[])).unwrap().1, InputEvent::ReleaseAll);
    }

    #[test]
    fn malformed_packets_are_rejected_without_panicking() {
        assert!(decode_input(&[]).is_none());
        assert!(decode_input(&packet(0, &[1, 2])).is_none());
        assert!(decode_input(&packet(9, &[])).is_none());
        assert!(decode_input(&packet(4, &[0xff, 0xfe])).is_none());
    }
}
