use super::*;

use std::{ffi::c_void, os::windows::io::FromRawHandle as _};

type Handle = *mut c_void;

const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x0000_0008;
const ERROR_PIPE_CONNECTED: i32 = 535;
const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_IMPERSONATION_LEVEL: i32 = 9;
const SECURITY_IDENTIFICATION_LEVEL: i32 = 1;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateNamedPipeW(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        max_instances: u32,
        out_buffer: u32,
        in_buffer: u32,
        default_timeout: u32,
        attributes: *const c_void,
    ) -> Handle;
    fn ConnectNamedPipe(pipe: Handle, overlapped: *mut c_void) -> i32;
    fn GetCurrentThread() -> Handle;
    fn CloseHandle(handle: Handle) -> i32;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn ImpersonateNamedPipeClient(pipe: Handle) -> i32;
    fn OpenThreadToken(thread: Handle, access: u32, open_as_self: i32, token: *mut Handle) -> i32;
    fn GetTokenInformation(
        token: Handle,
        class: i32,
        information: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
    fn RevertToSelf() -> i32;
}

/// Serves one connection on a fresh pipe the way the Windows agent would:
/// reads one byte, then reports the impersonation level the client granted.
fn impersonation_level_of_next_client(pipe: &str) -> thread::JoinHandle<i32> {
    let wide = pipe.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    // SAFETY: `wide` is NUL-terminated and outlives the call; null attributes
    // give the pipe the default security descriptor.
    let handle = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_REJECT_REMOTE_CLIENTS,
            1,
            1024,
            1024,
            0,
            std::ptr::null(),
        )
    };
    assert!(
        handle as isize != -1,
        "creating the test pipe: {}",
        io::Error::last_os_error()
    );
    let handle = handle as usize;
    thread::spawn(move || {
        let handle = handle as Handle;
        // SAFETY: this thread owns the pipe handle from here on.
        if unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } == 0 {
            assert_eq!(
                io::Error::last_os_error().raw_os_error(),
                Some(ERROR_PIPE_CONNECTED)
            );
        }
        let mut stream = unsafe { File::from_raw_handle(handle) };
        stream.read_exact(&mut [0_u8; 1]).unwrap();

        // SAFETY: every out-pointer is a live local of the matching size.
        unsafe {
            assert_ne!(ImpersonateNamedPipeClient(handle), 0, "impersonating");
            let mut token: Handle = std::ptr::null_mut();
            // As self: an identification-level token cannot be opened as itself.
            let opened = OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token);
            assert_ne!(RevertToSelf(), 0, "reverting");
            assert_ne!(opened, 0, "opening the impersonation token");
            let mut level = 0_i32;
            let mut length = 0_u32;
            let queried = GetTokenInformation(
                token,
                TOKEN_IMPERSONATION_LEVEL,
                (&raw mut level).cast(),
                size_of::<i32>() as u32,
                &mut length,
            );
            CloseHandle(token);
            assert_ne!(queried, 0, "reading the impersonation level");
            level
        }
    })
}

/// `SSH_AUTH_SOCK` names a pipe whoever created it serves: it may learn who
/// connected but not act as this process.
#[test]
fn the_agent_pipe_is_granted_identification_only() {
    let pipe = format!(
        r"\\.\pipe\wslx-sqos-test-{}-{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    );
    let agent = impersonation_level_of_next_client(&pipe);
    let mut stream = open_pipe(&pipe).expect("opening the agent pipe");
    stream.write_all(&[0]).unwrap();
    assert_eq!(agent.join().unwrap(), SECURITY_IDENTIFICATION_LEVEL);
}
