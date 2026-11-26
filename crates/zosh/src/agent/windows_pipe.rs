//! Creates a private Windows named pipe for SSH-agent forwarding.
//!
//! The name is random, but any account can list pipe names once they exist,
//! and a name with no instance left is free for any of them to create. So the
//! first instance refuses a name somebody else already holds
//! (`FILE_FLAG_FIRST_PIPE_INSTANCE`), and a listener creates each further
//! instance before it lets go of the previous one.
//!
//! `serve` is that listener, written once for the client's bootstrap relay and
//! the server's agent pipe.
//!
//! Every client end is opened through `open`, at identification level, so
//! whatever serves a name — a forwarded agent, a listener being woken — can
//! learn who connected but cannot act as this process.

use std::{
    fs::{File, OpenOptions},
    io,
    ops::ControlFlow,
    os::windows::{
        fs::OpenOptionsExt as _,
        io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle},
    },
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use windows::{
    Win32::{
        Foundation::{ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SDDL_REVISION_1,
            },
            GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
            TOKEN_USER, TokenUser,
        },
        Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION,
        },
        System::{
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
            },
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    },
    core::{BOOL, PCWSTR, PWSTR},
};

/// Opens the client end of the pipe at `path` for reading and writing, with
/// the server limited to identifying this process rather than impersonating
/// it. Without it a pipe client grants `SecurityImpersonation`.
pub(super) fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .security_qos_flags(SECURITY_IDENTIFICATION.0)
        .open(path)
}

/// A pipe's name as the wide, NUL-terminated string the API takes, which it
/// is by construction.
pub(super) struct PipeName(Vec<u16>);

impl PipeName {
    pub(super) fn new(name: &std::ffi::OsStr) -> Self {
        use std::os::windows::ffi::OsStrExt as _;
        Self(name.encode_wide().chain(std::iter::once(0)).collect())
    }

    fn as_pcwstr(&self) -> PCWSTR {
        PCWSTR(self.0.as_ptr())
    }
}

/// Creates an instance of `name`. `first_instance` refuses a name that already
/// exists, which is what a listener's first instance must do.
pub(super) fn create(
    name: &PipeName,
    buffer_size: u32,
    first_instance: bool,
) -> io::Result<OwnedHandle> {
    let sddl = user_only_descriptor()?
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut descriptor = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
    // SAFETY: `sddl` is NUL-terminated and outlives the call; `descriptor`
    // receives a descriptor this function frees below.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .map_err(io::Error::other)?;

    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: BOOL(0),
    };
    let open_mode = if first_instance {
        PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE
    } else {
        PIPE_ACCESS_DUPLEX
    };
    // SAFETY: `name` is a NUL-terminated wide string borrowed for the call,
    // and `attributes` points at `descriptor`, freed only after this.
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_pcwstr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            buffer_size,
            buffer_size,
            0,
            Some(&attributes),
        )
    };
    let created = if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: `CreateNamedPipeW` succeeded, so `handle` is a new instance
        // nothing else owns.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
    };
    // SAFETY: the conversion above allocated `descriptor` with `LocalAlloc`,
    // and the instance keeps its own copy.
    unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    created
}

/// Serves `name` from `pending`, its first instance, handing each client that
/// connects to `connected`, until `stop` is set or `connected` breaks.
///
/// The next instance is created before a connected one is handed over, so the
/// name always has an instance. If one cannot be created, the listener stops
/// rather than leave the name free for another account to create.
pub(super) fn serve(
    name: &PipeName,
    buffer_size: u32,
    mut pending: OwnedHandle,
    stop: &AtomicBool,
    mut connected: impl FnMut(File) -> ControlFlow<()>,
) {
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        // SAFETY: `pending` is an instance this loop owns, open until it is
        // replaced below.
        let result = unsafe { ConnectNamedPipe(HANDLE(pending.as_raw_handle()), None) };
        if stop.load(Ordering::Acquire) {
            return;
        }
        let Ok(next) = create(name, buffer_size, false) else {
            return;
        };
        let instance = std::mem::replace(&mut pending, next);
        let accepted = match result {
            Ok(()) => true,
            // The client connected between the instance being created and
            // `ConnectNamedPipe` being called.
            Err(error) => error.code().0 as u32 & 0xffff == ERROR_PIPE_CONNECTED.0,
        };
        // An instance whose connect failed is dropped, which closes it.
        if accepted && connected(File::from(instance)).is_break() {
            return;
        }
    }
}

/// A DACL granting this account and SYSTEM, by the account's own SID.
///
/// Not `OW` (owner rights): an elevated token's default owner is the
/// Administrators group, so a pipe created under an elevated SSH login refused
/// the same account's non-elevated processes — among them a `zmux` daemon
/// started from the desktop, which relays a remote pane's agent to this pipe.
fn user_only_descriptor() -> io::Result<String> {
    let mut token = HANDLE::default();
    // SAFETY: the current-process pseudo-handle needs no closing, and `token`
    // is written only on success.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(io::Error::other)?;
    // SAFETY: `OpenProcessToken` succeeded, so this process owns `token`.
    let owned = unsafe { OwnedHandle::from_raw_handle(token.0) };
    let token = HANDLE(owned.as_raw_handle());
    let mut length = 0_u32;
    // SAFETY: a size query: no buffer, and `length` receives the size needed.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut length) };
    // `u64`s, so the buffer is aligned for the `TOKEN_USER` read below.
    let mut buffer = vec![0_u64; (length as usize).div_ceil(8)];
    // SAFETY: `buffer` holds at least `length` bytes, the size just queried.
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            length,
            &mut length,
        )
    }
    .map_err(io::Error::other)?;
    // SAFETY: `GetTokenInformation` filled `buffer` with a `TOKEN_USER`, and
    // its SID points into the same buffer. `sid` is allocated by
    // `ConvertSidToStringSidW` and freed once copied out.
    unsafe {
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut sid = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut sid).map_err(io::Error::other)?;
        let text = sid.to_string().map_err(io::Error::other);
        LocalFree(Some(HLOCAL(sid.0.cast())));
        Ok(format!("D:P(A;;GA;;;{})(A;;GA;;;SY)", text?))
    }
}

/// Serves one connection on a fresh pipe at `path` the way an agent would:
/// reads `request_len` bytes, then reports the impersonation level the client
/// granted, then answers with `reply`.
#[cfg(test)]
pub(super) fn impersonation_level_of_next_client(
    path: &Path,
    request_len: usize,
    reply: Vec<u8>,
) -> std::thread::JoinHandle<windows::Win32::Security::SECURITY_IMPERSONATION_LEVEL> {
    use std::io::{Read as _, Write as _};
    use windows::Win32::{
        Foundation::ERROR_PIPE_CONNECTED,
        Security::{
            GetTokenInformation, RevertToSelf, SECURITY_IMPERSONATION_LEVEL, TOKEN_QUERY,
            TokenImpersonationLevel,
        },
        System::{
            Pipes::{ConnectNamedPipe, ImpersonateNamedPipeClient},
            Threading::{GetCurrentThread, OpenThreadToken},
        },
    };

    let instance =
        create(&PipeName::new(path.as_os_str()), 1024, true).expect("create test agent pipe");
    std::thread::spawn(move || {
        // SAFETY: `instance` is open until it becomes `stream` below.
        if let Err(error) = unsafe { ConnectNamedPipe(HANDLE(instance.as_raw_handle()), None) } {
            assert_eq!(error.code().0 as u32 & 0xffff, ERROR_PIPE_CONNECTED.0);
        }
        let mut stream = std::fs::File::from(instance);
        let mut request = vec![0_u8; request_len];
        stream.read_exact(&mut request).unwrap();

        // SAFETY: `stream` keeps the connected instance open.
        unsafe { ImpersonateNamedPipeClient(HANDLE(stream.as_raw_handle())) }
            .expect("impersonating the client");
        let mut token = HANDLE::default();
        // As self: an identification-level token cannot be opened as itself.
        // SAFETY: the current-thread pseudo-handle needs no closing, and
        // `token` is written only on success.
        let opened = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token) };
        // SAFETY: this thread impersonated the client just above.
        unsafe { RevertToSelf() }.expect("reverting");
        opened.expect("opening the impersonation token");
        // SAFETY: `OpenThreadToken` succeeded, so this thread owns `token`.
        let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
        let mut level = SECURITY_IMPERSONATION_LEVEL::default();
        let mut length = 0_u32;
        // SAFETY: `level` is exactly the size passed, and `token` is open.
        unsafe {
            GetTokenInformation(
                HANDLE(token.as_raw_handle()),
                TokenImpersonationLevel,
                Some((&raw mut level).cast()),
                std::mem::size_of::<SECURITY_IMPERSONATION_LEVEL>() as u32,
                &mut length,
            )
        }
        .expect("reading the impersonation level");

        stream.write_all(&reply).unwrap();
        level
    })
}
