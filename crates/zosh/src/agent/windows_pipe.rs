//! Creates a private Windows named pipe for SSH-agent forwarding.

use std::io;
use windows::{
    Win32::{
        Foundation::{HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree},
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::PIPE_ACCESS_DUPLEX,
        System::Pipes::{
            CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
            PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
        },
    },
    core::{BOOL, PCWSTR, w},
};

pub(super) fn create(name: PCWSTR, buffer_size: u32) -> io::Result<HANDLE> {
    let mut descriptor = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w!("D:P(A;;GA;;;OW)(A;;GA;;;SY)"),
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
    let handle = unsafe {
        CreateNamedPipeW(
            name,
            PIPE_ACCESS_DUPLEX,
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
