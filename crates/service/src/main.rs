//! `remotex-service.exe`
//!
//! * `service`   run as the Windows service (started by the Service Control Manager)
//! * `agent`     short-lived helper started by the service inside the user's session
//! * `install` / `uninstall`   used by the RemoteX installer (elevated)
//! * `console`   development only: the broker as an ordinary process, agent as the same user

#[cfg(windows)]
use remotex_service::{agent, broker, install, server, service};

#[cfg(windows)]
fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::EnvFilter;
    let dir =
        std::env::var_os("ProgramData").map(|p| std::path::PathBuf::from(p).join("RemoteX").join("logs"))?;
    std::fs::create_dir_all(&dir).ok()?;
    let file = tracing_appender::rolling::daily(dir, "service.log");
    let (writer, guard) = tracing_appender::non_blocking(file);
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    Some(guard)
}

#[cfg(windows)]
fn arg<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let _guard = init_logging();
    let code = match args.first().map(String::as_str) {
        Some("service") => service::run().map(|_| 0).unwrap_or_else(|e| {
            tracing::error!(error = %e, "service failed to start");
            1
        }),
        Some("agent") => match (arg(&args, "--input"), arg(&args, "--video")) {
            (Some(i), Some(v)) => agent::run(i, v).map(|_| 0).unwrap_or(1),
            _ => 2,
        },
        Some("install") => match install::install() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("install failed: {e}");
                1
            }
        },
        Some("uninstall") => match install::uninstall() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("uninstall failed: {e}");
                1
            }
        },
        Some("console") => console(&args),
        Some("--version") => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            0
        }
        _ => {
            eprintln!("usage: remotex-service (service | agent | install | uninstall)");
            2
        }
    };
    std::process::exit(code);
}

/// Development harness: never installed, never used in production.
#[cfg(windows)]
fn console(args: &[String]) -> i32 {
    use std::sync::{atomic::AtomicBool, Arc};
    let pipe = arg(args, "--pipe")
        .unwrap_or(remotex_elevate::SERVICE_PIPE)
        .to_string();
    let allow: Vec<std::path::PathBuf> = arg(args, "--allow").map(|p| vec![p.into()]).unwrap_or_default();
    let broker = broker::Broker::new(broker::BrokerConfig {
        exe: std::env::current_exe().expect("current exe"),
        allowed_ui: allow,
        launch: broker::LaunchKind::SameUser,
        enforce_console_session: false,
        require_protected_install: false,
    });
    server::serve(broker, Arc::new(AtomicBool::new(false)), &pipe)
        .map(|_| 0)
        .unwrap_or(1)
}

#[cfg(not(windows))]
fn main() {
    eprintln!("remotex-service is a Windows program");
}
