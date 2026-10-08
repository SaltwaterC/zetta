//! Creates a private Windows named pipe for SSH-agent forwarding.
//!
//! The name is random, but any account can list pipe names once they exist,
//! and a name with no instance left is free for any of them to create. So the
//! first instance refuses a name somebody else already holds
//! (`FILE_FLAG_FIRST_PIPE_INSTANCE`), and a listener creates each further
//! instance before it lets go of the previous one.

use std::io;
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SDDL_REVISION_1,
            },
            GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
            TOKEN_USER, TokenUser,
        },
        Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX},
        System::{
            Pipes::{
                CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
                PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
            },
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    },
    core::{BOOL, PCWSTR, PWSTR},
};

/// Creates an instance of `name`. `first_instance` refuses a name that already
/// exists, which is what a listener's first instance must do.
pub(super) fn create(name: PCWSTR, buffer_size: u32, first_instance: bool) -> io::Result<HANDLE> {
    let sddl = user_only_descriptor()?
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut descriptor = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
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
    let handle = unsafe {
        CreateNamedPipeW(
            name,
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            buffer_size,
            buffer_size,
            0,
            Some(&attributes),
        )
    };
    let error = (handle == INVALID_HANDLE_VALUE).then(io::Error::last_os_error);
    unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    error.map_or(Ok(handle), Err)
}

/// A DACL granting this account and SYSTEM, by the account's own SID.
///
/// Not `OW` (owner rights): an elevated token's default owner is the
/// Administrators group, so a pipe created under an elevated SSH login refused
/// the same account's non-elevated processes — among them a `zmux` daemon
/// started from the desktop, which relays a remote pane's agent to this pipe.
fn user_only_descriptor() -> io::Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(io::Error::other)?;
        let mut length = 0_u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut length);
        // `u64`s, so the buffer is aligned for the `TOKEN_USER` read below.
        let mut buffer = vec![0_u64; (length as usize).div_ceil(8)];
        let queried = GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            length,
            &mut length,
        );
        let _ = CloseHandle(token);
        queried.map_err(io::Error::other)?;
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut sid = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut sid).map_err(io::Error::other)?;
        let text = sid.to_string().map_err(io::Error::other);
        LocalFree(Some(HLOCAL(sid.0.cast())));
        Ok(format!("D:P(A;;GA;;;{})(A;;GA;;;SY)", text?))
    }
}
