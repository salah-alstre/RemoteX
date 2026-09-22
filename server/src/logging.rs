use std::path::Path;
use tracing::level_filters::LevelFilter;
use tracing_appender::{non_blocking::WorkerGuard, rolling};
use tracing_subscriber::{filter::filter_fn, fmt, prelude::*, registry::Registry, Layer};

type BoxedLayer = Box<dyn Layer<Registry> + Send + Sync>;

/// Three JSON log streams: `server` (application), `connection`, `security`. Secrets are never logged.
pub fn init(dir: &Path, console: bool) -> Vec<WorkerGuard> {
    let _ = std::fs::create_dir_all(dir);
    let (app, g1) = tracing_appender::non_blocking(rolling::daily(dir, "server.log"));
    let (conn, g2) = tracing_appender::non_blocking(rolling::daily(dir, "connection.log"));
    let (sec, g3) = tracing_appender::non_blocking(rolling::daily(dir, "security.log"));

    let json = |w| fmt::layer().json().with_ansi(false).with_writer(w);
    let mut layers: Vec<BoxedLayer> = vec![
        json(app)
            .with_filter(filter_fn(|m| !matches!(m.target(), "connection" | "security")))
            .boxed(),
        json(conn)
            .with_filter(filter_fn(|m| m.target() == "connection"))
            .boxed(),
        json(sec)
            .with_filter(filter_fn(|m| m.target() == "security"))
            .boxed(),
    ];
    if console {
        layers.push(
            fmt::layer()
                .with_target(true)
                .with_filter(LevelFilter::INFO)
                .boxed(),
        );
    }
    let _ = tracing_subscriber::registry().with(layers).try_init();
    vec![g1, g2, g3]
}
