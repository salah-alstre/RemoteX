//! Named-pipe plumbing with explicit access control (Windows only).
//!
//! Every server pipe is created with a discretionary ACL written out in SDDL, remote clients are
//! refused, and the very first instance uses `FILE_FLAG_FIRST_PIPE_INSTANCE` so nobody can squat the
//! name before the service creates it.

use std::{
    fs::{File, OpenOptions},
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, RawHandle},
    path::PathBuf,
    time::{Duration, Instant},
};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{CloseHandle, LocalFree, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL},
        Security::{
            Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX},
        System::{
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
                GetNamedPipeClientSessionId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
                PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
            },
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
    },
};

/// SYSTEM and administrators may do anything; the interactively logged-on user may read and write.
pub const USER_PIPE_SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)";
/// Only SYSTEM (the service and the agent it starts).
pub const SYSTEM_ONLY_SDDL: &str = "D:(A;;GA;;;SY)";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn win_err(e: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(e.code().0 & 0xffff)
}

/// An access-control list built from SDDL, kept alive for as long as pipes are created with it.
pub struct PipeSecurity {
    descriptor: PSECURITY_DESCRIPTOR,
    attrs: SECURITY_ATTRIBUTES,
}

// SAFETY: the descriptor is immutable after construction and only read by the kernel.
unsafe impl Send for PipeSecurity {}
unsafe impl Sync for PipeSecurity {}

impl PipeSecurity {
    pub fn from_sddl(sddl: &str) -> io::Result<Self> {
        let w = wide(sddl);
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `w` is a NUL-terminated UTF-16 string; the descriptor is freed in Drop.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(w.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }
        .map_err(win_err)?;
        let attrs = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        Ok(Self { descriptor, attrs })
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.descriptor.0)));
        }
    }
}

/// Creates one instance of a server pipe.
pub fn create_server(name: &str, security: &PipeSecurity, first: bool) -> io::Result<File> {
    let w = wide(name);
    let mut open_mode = PIPE_ACCESS_DUPLEX;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // SAFETY: valid NUL-terminated name and security attributes that outlive the call.
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(w.as_ptr()),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            1 << 20,
            1 << 20,
            0,
            Some(&security.attrs),
        )
    };
    if handle.is_invalid() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the handle is valid, owned by us, and now owned by the File.
    Ok(unsafe { File::from_raw_handle(handle.0 as RawHandle) })
}

fn raw(f: &File) -> HANDLE {
    HANDLE(f.as_raw_handle() as *mut _)
}

/// Blocks until a client connects.
pub fn accept(server: &File) -> io::Result<()> {
    // SAFETY: the handle belongs to `server`.
    match unsafe { ConnectNamedPipe(raw(server), None) } {
        Ok(()) => Ok(()),
        Err(e) if (e.code().0 & 0xffff) as u32 == ERROR_PIPE_CONNECTED.0 => Ok(()),
        Err(e) => Err(win_err(e)),
    }
}

pub fn disconnect(server: &File) {
    // SAFETY: the handle belongs to `server`.
    unsafe {
        let _ = DisconnectNamedPipe(raw(server));
    }
}

pub fn client_pid(server: &File) -> io::Result<u32> {
    let mut pid = 0u32;
    // SAFETY: valid handle and out pointer.
    unsafe { GetNamedPipeClientProcessId(raw(server), &mut pid) }.map_err(win_err)?;
    Ok(pid)
}

pub fn client_session(server: &File) -> io::Result<u32> {
    let mut id = 0u32;
    // SAFETY: valid handle and out pointer.
    unsafe { GetNamedPipeClientSessionId(raw(server), &mut id) }.map_err(win_err)?;
    Ok(id)
}

/// Full path of the executable a process was started from.
pub fn process_image(pid: u32) -> io::Result<PathBuf> {
    // SAFETY: the process handle is closed before returning; the buffer is sized before the call.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).map_err(win_err)?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.map_err(win_err)?;
        Ok(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// Connects to a pipe, waiting briefly if every instance is busy.
pub fn connect(name: &str, wait: Duration) -> io::Result<File> {
    let deadline = Instant::now() + wait;
    loop {
        match OpenOptions::new().read(true).write(true).open(name) {
            Ok(f) => return Ok(f),
            Err(e) if Instant::now() < deadline && matches!(e.raw_os_error(), Some(2) | Some(231)) => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{read_msg, write_msg, UiToService};

    #[test]
    fn pipe_round_trip_and_client_identity() {
        let name = format!(r"\\.\pipe\remotex-test-{}", std::process::id());
        let sec = PipeSecurity::from_sddl(USER_PIPE_SDDL).expect("sddl");
        let server = create_server(&name, &sec, true).expect("first instance");
        let n2 = name.clone();
        let client = std::thread::spawn(move || {
            let mut c = connect(&n2, Duration::from_secs(2)).unwrap();
            write_msg(&mut c, &UiToService::Heartbeat).unwrap();
        });
        accept(&server).unwrap();
        // The service identifies the caller from the pipe, not from anything the caller says.
        assert_eq!(client_pid(&server).unwrap(), std::process::id());
        let mut s = &server;
        assert!(matches!(
            read_msg::<_, UiToService>(&mut s).unwrap(),
            UiToService::Heartbeat
        ));
        client.join().unwrap();
        // The name cannot be claimed twice as the first instance.
        assert!(create_server(&name, &sec, true).is_err());
    }

    #[test]
    fn process_image_reports_this_executable() {
        let img = process_image(std::process::id()).unwrap();
        assert_eq!(img, std::env::current_exe().unwrap());
    }
}
