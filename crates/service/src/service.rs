//! Windows Service host: starts at boot, restarted by the Service Control Manager after a crash.

use crate::{
    broker::{Broker, BrokerConfig, LaunchKind},
    server,
};
use remotex_elevate::{SERVICE_NAME, SERVICE_PIPE};
use std::{
    ffi::OsString,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

define_windows_service!(ffi_service_main, service_main);

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
    Ok(())
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        tracing::error!(error = %e, "service terminated with error");
    }
}

fn status(state: ServiceState, accept: ServiceControlAccept, code: u32) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accept,
        exit_code: ServiceExitCode::Win32(code),
        checkpoint: 0,
        wait_hint: Duration::from_secs(10),
        process_id: None,
    }
}

fn run_service() -> Result<(), Box<dyn std::error::Error>> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_h = stop.clone();
    let handle = service_control_handler::register(SERVICE_NAME, move |ctl| match ctl {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            stop_h.store(true, std::sync::atomic::Ordering::SeqCst);
            server::wake(SERVICE_PIPE);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    handle.set_service_status(status(
        ServiceState::StartPending,
        ServiceControlAccept::empty(),
        0,
    ))?;

    let exe = std::env::current_exe()?;
    let allowed = exe
        .parent()
        .map(|d| vec![d.join("remotex.exe")])
        .unwrap_or_default();
    let broker = Broker::new(BrokerConfig {
        exe,
        allowed_ui: allowed,
        launch: LaunchKind::System,
        enforce_console_session: true,
        require_protected_install: true,
    });
    handle.set_service_status(status(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        0,
    ))?;
    tracing::info!("RemoteX service started");
    let outcome = server::serve(broker, stop, SERVICE_PIPE);
    let code = if let Err(e) = &outcome {
        tracing::error!(error = %e, "pipe server stopped");
        1
    } else {
        0
    };
    handle.set_service_status(status(ServiceState::Stopped, ServiceControlAccept::empty(), code))?;
    Ok(())
}
