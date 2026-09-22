//! Windows Service host so the server starts at boot and is restarted by the Service Control Manager.

use anyhow::Result;
use remotex_server::{config::Config, logging, serve};
use std::{ffi::OsString, net::TcpListener, sync::Arc, time::Duration};
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

const SERVICE_NAME: &str = "RemoteXServer";

define_windows_service!(ffi_service_main, service_main);

pub fn run() -> Result<()> {
    // Services start with C:\Windows\System32 as the working directory; anchor on the executable instead.
    if let Some(dir) = std::env::current_exe()?.parent() {
        std::env::set_current_dir(dir)?;
    }
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
    Ok(())
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        tracing::error!(error = %e, "service terminated with error");
    }
}

fn run_service() -> Result<()> {
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let stop_tx = std::sync::Mutex::new(Some(stop_tx));
    let status_handle = service_control_handler::register(SERVICE_NAME, move |ctl| match ctl {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            if let Some(tx) = stop_tx.lock().ok().and_then(|mut g| g.take()) {
                let _ = tx.send(());
            }
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;

    let status = |state, accept, code: u32, wait: u64| ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accept,
        exit_code: ServiceExitCode::Win32(code),
        checkpoint: 0,
        wait_hint: Duration::from_secs(wait),
        process_id: None,
    };
    status_handle.set_service_status(status(
        ServiceState::StartPending,
        ServiceControlAccept::empty(),
        0,
        10,
    ))?;

    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            status_handle.set_service_status(status(
                ServiceState::Stopped,
                ServiceControlAccept::empty(),
                1,
                0,
            ))?;
            return Err(e);
        }
    };
    let _guards = logging::init(&cfg.log_dir, false);
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let listener = TcpListener::bind((cfg.host.as_str(), cfg.port))?;
    status_handle.set_service_status(status(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        0,
        0,
    ))?;

    let result = rt.block_on(serve(Arc::new(cfg), listener, async {
        let _ = stop_rx.await;
    }));

    let code = if result.is_ok() { 0 } else { 1 };
    status_handle.set_service_status(status(
        ServiceState::Stopped,
        ServiceControlAccept::empty(),
        code,
        0,
    ))?;
    result
}
