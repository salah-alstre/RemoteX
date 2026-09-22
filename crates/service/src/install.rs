//! Installing and removing the Windows service. Invoked by the RemoteX installer (elevated); users never
//! run this by hand.

use remotex_elevate::{SERVICE_DISPLAY_NAME, SERVICE_NAME};
use std::{ffi::OsString, process::Command, time::Duration};
use windows_service::{
    service::{
        ServiceAccess, ServiceAction, ServiceActionType, ServiceErrorControl, ServiceFailureActions,
        ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState, ServiceType,
    },
    service_manager::{ServiceManager, ServiceManagerAccess},
};

/// Who may do what to the service itself: SYSTEM and administrators manage it; ordinary users may only
/// see that it exists and query its state. Nobody else can stop, reconfigure or delete it.
const SERVICE_SDDL: &str = "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWLOCRRC;;;IU)(A;;CCLCSWLOCRRC;;;SU)";

pub fn install() -> Result<(), Box<dyn std::error::Error>> {
    let exe = std::env::current_exe()?;
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )?;
    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec![OsString::from("service")],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let access = ServiceAccess::ALL_ACCESS;
    let service = match manager.open_service(SERVICE_NAME, access) {
        Ok(existing) => {
            stop_quietly(&existing);
            existing.change_config(&info)?;
            existing
        }
        Err(_) => manager.create_service(&info, access)?,
    };
    service.set_description(
        "Lets an approved RemoteX session interact with administrator windows and UAC prompts on this PC.",
    )?;
    // Restart after crashes: 5 s, 5 s, then 30 s; the counter resets after a day of stability.
    let restart = |secs| ServiceAction {
        action_type: ServiceActionType::Restart,
        delay: Duration::from_secs(secs),
    };
    service.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86_400)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![restart(5), restart(5), restart(30)]),
    })?;
    service.set_failure_actions_on_non_crash_failures(true)?;
    // Restrictive access list for the service object itself.
    let sd = Command::new("sc.exe")
        .args(["sdset", SERVICE_NAME, SERVICE_SDDL])
        .output()?;
    if !sd.status.success() {
        return Err(format!(
            "could not restrict the service: {}",
            String::from_utf8_lossy(&sd.stdout)
        )
        .into());
    }
    let _ = service.start::<&str>(&[]);
    Ok(())
}

fn stop_quietly(service: &windows_service::service::Service) {
    if let Ok(status) = service.query_status() {
        if status.current_state != ServiceState::Stopped {
            let _ = service.stop();
            for _ in 0..40 {
                std::thread::sleep(Duration::from_millis(250));
                if service
                    .query_status()
                    .is_ok_and(|s| s.current_state == ServiceState::Stopped)
                {
                    break;
                }
            }
        }
    }
}

pub fn uninstall() -> Result<(), Box<dyn std::error::Error>> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let Ok(service) = manager.open_service(SERVICE_NAME, ServiceAccess::ALL_ACCESS) else {
        return Ok(()); // nothing to remove
    };
    stop_quietly(&service);
    service.delete()?;
    Ok(())
}
