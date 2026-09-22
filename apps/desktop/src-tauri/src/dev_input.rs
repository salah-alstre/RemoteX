//! Development-only input sink: appends each applied event, with a wall-clock timestamp in
//! microseconds, to a file. Never compiled into release installers (`dev-tools` feature).

use remotex_common::peer::{DisplayInfo, InputEvent};
use remotex_session::env::InputSink;
use std::{fs::File, io::Write, path::Path, time::SystemTime};

pub struct LogInput(Option<File>);

impl LogInput {
    pub fn open(path: &Path) -> Self {
        Self(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok(),
        )
    }
}

impl InputSink for LogInput {
    fn apply(&mut self, event: &InputEvent, _display: &DisplayInfo) {
        let us = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_micros())
            .unwrap_or(0);
        if let Some(f) = self.0.as_mut() {
            let _ = writeln!(f, "{us} {event:?}");
        }
    }

    fn release_all(&mut self) {}
}
