//! The named-pipe half of a pane's agent name: one listener thread per pane,
//! one relay thread per connection.
//!
//! Listeners are kept in a process-wide table rather than on the pane, because
//! a pane is discarded from several places that only have its id, and because
//! an in-place upgrade hands the pane to a new process that has to serve the
//! same name again ([`serve`] from `upgrade::adopt_handover`).

use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io,
    os::windows::{ffi::OsStrExt as _, io::FromRawHandle as _},
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL,
            INVALID_HANDLE_VALUE, LocalFree,
        },
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SDDL_REVISION_1,
            },
            GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
            TOKEN_USER, TokenUser,
        },
        Storage::FileSystem::PIPE_ACCESS_DUPLEX,
        System::{
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
            },
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    },
    core::{BOOL, PCWSTR, PWSTR},
};

use super::{MAX_FRAME, relay_frames, upstream_candidates};

/// How long a connection waits for a busy upstream agent to free an instance.
const UPSTREAM_BUSY_WAIT_MS: u32 = 2_000;

struct PaneListener {
    stop: Arc<AtomicBool>,
}

static LISTENERS: LazyLock<Mutex<HashMap<u64, PaneListener>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Starts serving `pane_id`'s agent pipe, unless it is already served.
///
/// `fallback` is the agent the pane would have used without Zosh. Failure is
/// logged rather than returned: a pane without agent forwarding still works.
pub(in crate::server) fn serve(pane_id: u64, fallback: Option<PathBuf>) {
    let mut listeners = LISTENERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if listeners.contains_key(&pane_id) {
        return;
    }
    let name = crate::paths::pane_forwarded_agent_pipe(pane_id);
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let spawned = thread::Builder::new()
        .name(format!("zmux-agent-pipe-{pane_id}"))
        .spawn({
            let stop = Arc::clone(&stop);
            move || accept_loop(pane_id, &name, fallback, &stop, ready_tx)
        });
    let ready = match spawned {
        Ok(_) => ready_rx
            .recv()
            .unwrap_or_else(|_| Err(io::Error::other("the agent pipe listener exited"))),
        Err(error) => Err(error),
    };
    match ready {
        Ok(()) => {
            listeners.insert(pane_id, PaneListener { stop });
        }
        Err(error) => log::warn!("could not serve pane {pane_id}'s agent pipe: {error}"),
    }
}

/// Stops serving `pane_id`'s agent pipe and forgets its published target.
///
/// Only for a pane the daemon is discarding: an upgrade hands live panes to its
/// replacement, which must find the target still in place.
pub(in crate::server) fn stop(pane_id: u64) {
    let listener = LISTENERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&pane_id);
    if let Some(listener) = listener {
        listener.stop.store(true, Ordering::Release);
        // The listener creates each instance before checking the flag, so an
        // open from here either finds that instance and wakes its blocked
        // connect, or finds none because the listener has not yet looked.
        let _ = OpenOptions::new()
            .read(true)
            .write(true)
            .open(crate::paths::pane_forwarded_agent_pipe(pane_id));
    }
    let target = crate::paths::pane_forwarded_agent_target(pane_id);
    if let Err(error) = std::fs::remove_file(&target)
        && error.kind() != io::ErrorKind::NotFound
    {
        log::debug!("could not remove {}: {error}", target.display());
    }
}

fn accept_loop(
    pane_id: u64,
    name: &Path,
    fallback: Option<PathBuf>,
    stop: &AtomicBool,
    ready: std::sync::mpsc::SyncSender<io::Result<()>>,
) {
    let wide = wide(name);
    let mut ready = Some(ready);
    let descriptor = match user_only_descriptor() {
        Ok(descriptor) => descriptor,
        Err(error) => {
            if let Some(ready) = ready.take() {
                let _ = ready.send(Err(error));
            }
            return;
        }
    };
    loop {
        let handle = match create_instance(&wide, &descriptor) {
            Ok(handle) => handle,
            Err(error) => {
                match ready.take() {
                    Some(ready) => {
                        let _ = ready.send(Err(error));
                    }
                    None => log::warn!("pane {pane_id}'s agent pipe stopped listening: {error}"),
                }
                return;
            }
        };
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok(()));
        }
        if stop.load(Ordering::Acquire) {
            let _ = unsafe { CloseHandle(handle) };
            return;
        }
        let connected = unsafe { ConnectNamedPipe(handle, None) };
        if let Err(error) = connected
            && error.code().0 as u32 & 0xffff != ERROR_PIPE_CONNECTED.0
        {
            let _ = unsafe { CloseHandle(handle) };
            continue;
        }
        let client = unsafe { File::from_raw_handle(handle.0 as _) };
        if stop.load(Ordering::Acquire) {
            return;
        }
        let fallback = fallback.clone();
        let _ = thread::Builder::new()
            .name(format!("zmux-agent-relay-{pane_id}"))
            .spawn(move || relay_connection(pane_id, client, fallback.as_deref()));
    }
}

fn relay_connection(pane_id: u64, mut client: File, fallback: Option<&Path>) {
    let published =
        std::fs::read_to_string(crate::paths::pane_forwarded_agent_target(pane_id)).ok();
    let mut last_error = None;
    for candidate in upstream_candidates(published.as_deref(), fallback) {
        match connect_upstream(&candidate) {
            Ok(mut agent) => {
                if let Err(error) = relay_frames(&mut client, &mut agent) {
                    log::debug!(
                        "pane {pane_id}'s agent relay to {} ended: {error}",
                        candidate.display()
                    );
                }
                return;
            }
            Err(error) => last_error = Some((candidate, error)),
        }
    }
    if let Some((candidate, error)) = last_error {
        log::debug!(
            "pane {pane_id} has no reachable agent; last tried {}: {error}",
            candidate.display()
        );
    }
}

fn connect_upstream(path: &Path) -> io::Result<File> {
    match OpenOptions::new().read(true).write(true).open(path) {
        Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => {
            let wide = wide(path);
            let _ = unsafe { WaitNamedPipeW(PCWSTR(wide.as_ptr()), UPSTREAM_BUSY_WAIT_MS) };
            OpenOptions::new().read(true).write(true).open(path)
        }
        result => result,
    }
}

fn create_instance(name: &[u16], descriptor: &str) -> io::Result<HANDLE> {
    let sddl = descriptor
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut security = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut security,
            None,
        )
    }
    .map_err(io::Error::other)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security.0,
        bInheritHandle: BOOL(0),
    };
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            MAX_FRAME as u32,
            MAX_FRAME as u32,
            0,
            Some(&attributes),
        )
    };
    let error = (handle == INVALID_HANDLE_VALUE).then(io::Error::last_os_error);
    unsafe { LocalFree(Some(HLOCAL(security.0))) };
    error.map_or(Ok(handle), Err)
}

/// A DACL granting this account and SYSTEM, by the account's own SID.
///
/// Not `OW` (owner rights): an elevated token's default owner is the
/// Administrators group, so a pipe created from an elevated SSH login would
/// refuse the same account's non-elevated processes, and the reverse.
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

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
