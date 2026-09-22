//! Panic handling: every panic is written to a crash log and the user is told what happened.

use crate::strings::{tr, Key, Lang};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONERROR, MB_TOPMOST, MB_YESNO},
    },
};

static SHOWING: AtomicBool = AtomicBool::new(false);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn write_crash_log(dir: &Path, info: &std::panic::PanicHookInfo<'_>) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("crash-{stamp}.log"));
    let backtrace = std::backtrace::Backtrace::force_capture();
    // Panic messages come from our own code and never contain credentials or clipboard data.
    let body = format!("{info}\n\n{backtrace}\n");
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&path, body);
    path
}

pub fn install(logs_dir: PathBuf, lang: Lang) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let path = write_crash_log(&logs_dir, info);
        tracing::error!(crash_log = %path.display(), "panic: {info}");
        if SHOWING.swap(true, Ordering::SeqCst) {
            return;
        }
        let title = wide(tr(lang, Key::CrashTitle));
        let body = wide(tr(lang, Key::CrashBody));
        // SAFETY: both buffers are NUL terminated and outlive the call.
        let choice = unsafe {
            MessageBoxW(
                Some(HWND::default()),
                PCWSTR(body.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_YESNO | MB_ICONERROR | MB_TOPMOST,
            )
        };
        if choice == IDYES {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).spawn();
            }
            std::process::exit(1);
        } else {
            let _ = std::process::Command::new("explorer").arg(&logs_dir).spawn();
        }
        SHOWING.store(false, Ordering::SeqCst);
    }));
}
