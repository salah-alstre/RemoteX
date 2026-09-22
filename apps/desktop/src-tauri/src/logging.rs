use std::path::Path;
use tracing::level_filters::LevelFilter;
use tracing_appender::{non_blocking::WorkerGuard, rolling};
use tracing_subscriber::{filter::filter_fn, fmt, prelude::*, registry::Registry, Layer};

type BoxedLayer = Box<dyn Layer<Registry> + Send + Sync>;

/// Log categories (the `target` of every event). Anything else goes to `app.log`.
const SECURITY: &[&str] = &["security"];
const SESSION: &[&str] = &["session"];
const NETWORK: &[&str] = &["network", "server_connection"];
const MEDIA: &[&str] = &["capture", "codec"];

/// JSON logs per category: `security`, `session`, `network` (+ server connection), `media`
/// (capture + codec) and `app`. Passwords, keys, tokens and clipboard content are never passed to the logger.
pub fn init(dir: &Path) -> Vec<WorkerGuard> {
    let _ = std::fs::create_dir_all(dir);
    let mut guards = Vec::new();
    let mut layers: Vec<BoxedLayer> = Vec::new();
    let mut file_layer = |name: &str, targets: &'static [&'static str]| {
        let (writer, guard) = tracing_appender::non_blocking(rolling::daily(dir, name));
        guards.push(guard);
        fmt::layer()
            .json()
            .with_ansi(false)
            .with_writer(writer)
            .with_filter(filter_fn(move |m| targets.contains(&m.target())))
            .boxed()
    };
    layers.push(file_layer("security.log", SECURITY));
    layers.push(file_layer("session.log", SESSION));
    layers.push(file_layer("network.log", NETWORK));
    layers.push(file_layer("media.log", MEDIA));

    let (app, guard) = tracing_appender::non_blocking(rolling::daily(dir, "app.log"));
    guards.push(guard);
    layers.push(
        fmt::layer()
            .json()
            .with_ansi(false)
            .with_writer(app)
            .with_filter(filter_fn(|m| {
                ![SECURITY, SESSION, NETWORK, MEDIA]
                    .iter()
                    .any(|group| group.contains(&m.target()))
            }))
            .with_filter(LevelFilter::INFO)
            .boxed(),
    );
    if cfg!(debug_assertions) {
        layers.push(fmt::layer().with_filter(LevelFilter::INFO).boxed());
    }
    let _ = tracing_subscriber::registry().with(layers).try_init();
    guards
}
