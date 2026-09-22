//! System tray: quick actions and an always-visible indicator of an active remote session.

use crate::{
    state::{lock, AppState},
    strings::{tr, Key, Lang},
};
use remotex_common::peer::ClipboardPayload;
use remotex_input::{system_clipboard::SystemClipboard, ClipboardBackend};
use std::sync::Arc;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};

pub struct TrayHandles {
    pub tray: TrayIcon,
    pub status: MenuItem<tauri::Wry>,
    pub incoming: CheckMenuItem<tauri::Wry>,
    pub lang: Lang,
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

pub fn build(app: &AppHandle, state: &Arc<AppState>) -> tauri::Result<TrayHandles> {
    let lang = Lang::resolve(&lock(&state.settings).language);
    let incoming_on = lock(&state.settings).incoming_enabled;

    let status = MenuItem::with_id(app, "status", tr(lang, Key::StatusReady), false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", tr(lang, Key::Open), true, None::<&str>)?;
    let copy = MenuItem::with_id(app, "copy_id", tr(lang, Key::CopyId), true, None::<&str>)?;
    let incoming = CheckMenuItem::with_id(
        app,
        "incoming",
        tr(lang, Key::IncomingOn),
        true,
        incoming_on,
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(app, "settings", tr(lang, Key::Settings), true, None::<&str>)?;
    let exit = MenuItem::with_id(app, "exit", tr(lang, Key::Exit), true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[&status, &sep1, &open, &copy, &incoming, &settings, &sep2, &exit],
    )?;

    let st = state.clone();
    let tray = TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().expect("bundled icon"))
        .tooltip(&state.brand.name)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "settings" => {
                show_main(app);
                let _ = app.emit("navigate", "settings");
            }
            "copy_id" => {
                if let Some(id) = st.engine.my_id() {
                    if let Ok(mut c) = SystemClipboard::new(false) {
                        let _ = c.write(&ClipboardPayload::Text(id.as_str().to_string()));
                    }
                }
            }
            "incoming" => {
                let enabled = {
                    let mut s = lock(&st.settings);
                    s.incoming_enabled = !s.incoming_enabled;
                    s.incoming_enabled
                };
                st.save_settings();
                st.engine.set_incoming_enabled(enabled);
                let _ = app.emit("settings-changed", ());
            }
            "exit" => {
                st.engine.host_command(remotex_session::host::HostCmd::Kick);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(TrayHandles {
        tray,
        status,
        incoming,
        lang,
    })
}

impl TrayHandles {
    pub fn set_session_active(&self, active: bool, brand: &str) {
        let key = if active {
            Key::StatusSession
        } else {
            Key::StatusReady
        };
        let _ = self.status.set_text(tr(self.lang, key));
        let tip = if active {
            format!("{brand} — {}", tr(self.lang, Key::StatusSession))
        } else {
            brand.to_string()
        };
        let _ = self.tray.set_tooltip(Some(tip));
    }

    pub fn set_incoming(&self, on: bool) {
        let _ = self.incoming.set_checked(on);
    }
}
