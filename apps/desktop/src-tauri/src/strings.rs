//! Strings shown by native (non-webview) UI: tray menu and the crash dialog.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    Ar,
}

impl Lang {
    /// `setting` is `"en"`, `"ar"` or `"system"`.
    pub fn resolve(setting: &str) -> Self {
        match setting {
            "ar" => Lang::Ar,
            "en" => Lang::En,
            _ => match sys_locale::get_locale() {
                Some(l) if l.to_lowercase().starts_with("ar") => Lang::Ar,
                _ => Lang::En,
            },
        }
    }
}

#[derive(Clone, Copy)]
pub enum Key {
    Open,
    CopyId,
    IncomingOn,
    Settings,
    Exit,
    StatusReady,
    StatusSession,
    CrashTitle,
    CrashBody,
}

pub fn tr(lang: Lang, key: Key) -> &'static str {
    use Key::*;
    match (lang, key) {
        (Lang::En, Open) => "Open application",
        (Lang::En, CopyId) => "Copy device ID",
        (Lang::En, IncomingOn) => "Allow incoming connections",
        (Lang::En, Settings) => "Settings",
        (Lang::En, Exit) => "Exit",
        (Lang::En, StatusReady) => "Ready for connections",
        (Lang::En, StatusSession) => "● Remote session active",
        (Lang::En, CrashTitle) => "Unexpected error",
        (Lang::En, CrashBody) => {
            "The application encountered an unexpected error.\n\nChoose Yes to restart the application, or No to open the logs folder."
        }
        (Lang::Ar, Open) => "فتح التطبيق",
        (Lang::Ar, CopyId) => "نسخ معرّف الجهاز",
        (Lang::Ar, IncomingOn) => "السماح بالاتصالات الواردة",
        (Lang::Ar, Settings) => "الإعدادات",
        (Lang::Ar, Exit) => "خروج",
        (Lang::Ar, StatusReady) => "جاهز للاتصالات",
        (Lang::Ar, StatusSession) => "● جلسة بعيدة نشطة",
        (Lang::Ar, CrashTitle) => "خطأ غير متوقع",
        (Lang::Ar, CrashBody) => "واجه التطبيق خطأً غير متوقع.\n\nاختر «نعم» لإعادة تشغيل التطبيق، أو «لا» لفتح مجلد السجلات.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_translated() {
        use Key::*;
        for k in [
            Open,
            CopyId,
            IncomingOn,
            Settings,
            Exit,
            StatusReady,
            StatusSession,
            CrashTitle,
            CrashBody,
        ] {
            assert!(!tr(Lang::En, k).is_empty() && !tr(Lang::Ar, k).is_empty());
            assert_ne!(tr(Lang::En, k), tr(Lang::Ar, k));
        }
    }

    #[test]
    fn explicit_setting_overrides_system_language() {
        assert_eq!(Lang::resolve("ar"), Lang::Ar);
        assert_eq!(Lang::resolve("en"), Lang::En);
    }
}
