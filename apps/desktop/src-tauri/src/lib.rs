//! RemoteX desktop shell: hosts the engine, exposes commands to the UI, owns tray, logging and crash handling.

mod commands;
mod config;
mod crash;
#[cfg(feature = "dev-tools")]
mod dev_input;
mod infra;
mod logging;
mod state;
mod strings;
mod tray;

use remotex_common::branding::Branding;
use remotex_session::{
    platform,
    store::{DeviceBook, DpapiCredentialStore, SecretFile, Settings},
    types::Event,
    Engine,
};
use state::{lock, AppState};
use std::{
    sync::{atomic::Ordering, Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{ipc::InvokeResponseBody, AppHandle, Emitter, Manager, WindowEvent};

/// Maximum unacknowledged video frames before the shell sheds load and asks for a fresh keyframe.
/// Kept tiny on purpose: every frame waiting here is a frame of extra latency, and a newer one is coming.
const MAX_INFLIGHT_FRAMES: usize = 3;

fn video_packet(f: &remotex_common::peer::VideoFrame) -> Vec<u8> {
    let mut out = Vec::with_capacity(26 + f.data.len());
    out.push(u8::from(f.keyframe));
    out.push(match f.codec {
        remotex_common::peer::Codec::H264 => 0,
        remotex_common::peer::Codec::H265 => 1,
        remotex_common::peer::Codec::Av1 => 2,
    });
    out.extend_from_slice(&f.width.to_le_bytes());
    out.extend_from_slice(&f.height.to_le_bytes());
    out.extend_from_slice(&f.captured_ms.to_le_bytes());
    out.extend_from_slice(&f.seq.to_le_bytes());
    out.extend_from_slice(&f.data);
    out
}

fn forward(app: &AppHandle, state: &Arc<AppState>, last_key_request: &mut Instant, ev: Event) {
    match &ev {
        Event::Video { session, frame } => {
            let pending = state.inflight.load(Ordering::SeqCst);
            if pending >= MAX_INFLIGHT_FRAMES && !frame.keyframe {
                state.stale_video.fetch_add(1, Ordering::Relaxed);
                if last_key_request.elapsed() > Duration::from_millis(500) {
                    *last_key_request = Instant::now();
                    state
                        .engine
                        .viewer_command(*session, remotex_session::viewer::ViewerCmd::RequestKeyframe);
                }
                return;
            }
            if let Some(ch) = lock(&state.video).as_ref() {
                if ch.send(InvokeResponseBody::Raw(video_packet(frame))).is_ok() {
                    state.inflight.fetch_add(1, Ordering::SeqCst);
                }
            }
            return;
        }
        Event::Incoming(_) => tray::show_main(app),
        Event::HostStarted { .. } | Event::HostEnded { .. } => {
            let active = matches!(ev, Event::HostStarted { .. });
            if let Some(t) = lock(&state.tray).as_ref() {
                t.set_session_active(active, &state.brand.name);
            }
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_title(&if active {
                    format!("{} — ●", state.brand.name)
                } else {
                    state.brand.name.clone()
                });
            }
        }
        _ => {}
    }
    if let Event::ViewerStats { session, stats } = &ev {
        // The shell owns the last hop, so it adds what it dropped there.
        let mut stats = stats.clone();
        stats.latency.stale_video_dropped = state.stale_video.load(Ordering::Relaxed);
        let _ = app.emit(
            "engine-event",
            &Event::ViewerStats {
                session: *session,
                stats,
            },
        );
        return;
    }
    let _ = app.emit("engine-event", &ev);
}

/// Developer-only: several instances side by side (test builds with the `dev-tools` feature).
fn dev_multi_instance() -> bool {
    cfg!(feature = "dev-tools") && std::env::var_os("REMOTEX_MULTI_INSTANCE").is_some()
}

/// The platform services for this build. Only development builds may point the elevated-control client at
/// another pipe (for testing against a service that is not installed); release builds always use the real one.
fn platform_env() -> remotex_session::env::PlatformEnv {
    #[cfg(feature = "dev-tools")]
    {
        let mut env = platform::env();
        if let Some(pipe) = std::env::var_os("REMOTEX_DEV_SERVICE_PIPE") {
            env = platform::env_with_service_pipe(&pipe.to_string_lossy());
        }
        // Records applied input to a file instead of moving the real mouse, so full-stack latency tests
        // can run on a developer machine.
        if let Some(path) = std::env::var_os("REMOTEX_DEV_INPUT_LOG") {
            env.input = Arc::new(move || Box::new(dev_input::LogInput::open(std::path::Path::new(&path))));
        }
        env
    }
    #[cfg(not(feature = "dev-tools"))]
    platform::env()
}

/// Developer-only: isolated per-instance data directory.
fn dev_data_dir() -> Option<std::ffi::OsString> {
    if cfg!(feature = "dev-tools") {
        std::env::var_os("REMOTEX_DATA_DIR")
    } else {
        None
    }
}

pub fn run() {
    let brand = Branding::load();
    let mut builder = tauri::Builder::default();
    // Running several instances side by side is only useful for testing two clients on one machine.
    if !dev_multi_instance() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main(app)
        }));
    }
    let result = builder
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .setup(move |app| {
            let (data_dir, logs_dir) = match dev_data_dir() {
                Some(dir) => (
                    std::path::PathBuf::from(&dir).join("data"),
                    std::path::PathBuf::from(dir).join("logs"),
                ),
                None => (
                    app.path().app_data_dir()?,
                    app.path().app_local_data_dir()?.join("logs"),
                ),
            };
            std::fs::create_dir_all(&data_dir)?;
            let guards = logging::init(&logs_dir);
            app.manage(guards);

            let settings = Settings::load(&data_dir.join("settings.json")).sanitized();
            crash::install(logs_dir.clone(), strings::Lang::resolve(&settings.language));
            let book = DeviceBook::load(&data_dir.join("devices.json"));

            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let store = Arc::new(DpapiCredentialStore::new(data_dir.join("identity.bin")));
            let cfg = config::engine_config(&settings);
            let engine = tauri::async_runtime::block_on(Engine::start(cfg, platform_env(), store, tx))?;

            let state = Arc::new(AppState {
                engine,
                data_dir: data_dir.clone(),
                logs_dir,
                settings: Mutex::new(settings.clone()),
                book: Mutex::new(book),
                secret: SecretFile::new(data_dir.join("unattended.bin")),
                brand: brand.clone(),
                video: Mutex::new(None),
                inflight: std::sync::atomic::AtomicUsize::new(0),
                stale_video: std::sync::atomic::AtomicU64::new(0),
                tray: Mutex::new(None),
            });
            commands::restore_unattended(&state);
            app.manage(state.clone());

            *lock(&state.tray) = Some(tray::build(app.handle(), &state)?);
            let (handle, st) = (app.handle().clone(), state.clone());
            tauri::async_runtime::spawn(async move {
                let mut last_key = Instant::now();
                while let Some(ev) = rx.recv().await {
                    forward(&handle, &st, &mut last_key, ev);
                }
            });

            let minimized = std::env::args().any(|a| a == "--minimized") || settings.launch_minimized;
            if minimized {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            tracing::info!("application started");
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if let Some(state) = window.try_state::<Arc<AppState>>() {
                    let (behavior, tray_ok) = {
                        let s = lock(&state.settings);
                        (s.close_behavior.clone(), s.minimize_to_tray)
                    };
                    if behavior == "tray" && tray_ok {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_bootstrap,
            commands::settings_save,
            commands::unattended_set_password,
            commands::unattended_clear_password,
            commands::regenerate_password,
            commands::connect,
            commands::viewer_input,
            commands::viewer_input_bin,
            commands::elevation_available,
            commands::viewer_select_display,
            commands::viewer_set_quality,
            commands::viewer_keyframe,
            commands::viewer_clipboard_sync,
            commands::viewer_chat,
            commands::viewer_send_files,
            commands::viewer_download,
            commands::viewer_list_dir,
            commands::viewer_file,
            commands::viewer_disconnect,
            commands::respond_incoming,
            commands::host_set_permissions,
            commands::host_chat,
            commands::host_kick,
            commands::host_approve_file,
            commands::book_trust,
            commands::book_revoke,
            commands::book_revoke_all,
            commands::book_rename_trusted,
            commands::book_upsert,
            commands::book_remove,
            commands::presence_query,
            commands::video_subscribe,
            commands::video_ack,
            commands::open_folder,
            commands::read_security_log,
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        tracing::error!(error = %e, "fatal error while running the application");
        panic!("application failed to run: {e}");
    }
}
