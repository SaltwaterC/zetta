//! The named-pipe half of a pane's agent name: one listener thread per pane,
//! one relay thread per connection.
//!
//! Listeners are kept in a process-wide table rather than on the pane, because
//! a pane is discarded from several places that only have its id, and because
//! an in-place upgrade hands the pane to a new process that has to serve the
//! same name again ([`adopt`] from `upgrade::adopt_handover`).
//!
//! A named pipe belongs to whoever creates its first instance: later instances
//! join that pipe and its DACL, and any account may create a name nobody holds.
//! So a pane's name is never left without an instance while the pane lives.
//! The first is created with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a name somebody
//! else already holds is refused rather than joined; each later instance is
//! created *before* the previous one is handed to a relay or closed; and an
//! upgrade passes the replacement an instance of its own ([`hand_over`]), which
//! keeps the name held across the moment the old daemon exits.

use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io,
    os::windows::{ffi::OsStrExt as _, fs::OpenOptionsExt as _, io::FromRawHandle as _},
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use windows::{
    Win32::{
        Foundation::{
            CloseHandle, DUPLICATE_CLOSE_SOURCE, DUPLICATE_SAME_ACCESS, DuplicateHandle,
            ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL,
            INVALID_HANDLE_VALUE, LocalFree,
        },
        Security::{
            ACL,
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
            },
            DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetTokenInformation, IsWellKnownSid,
            OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
            TOKEN_INFORMATION_CLASS, TOKEN_OWNER, TOKEN_QUERY, TOKEN_USER, TokenOwner, TokenUser,
            WinLocalSystemSid,
        },
        Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION,
        },
        System::{
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
                PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
                WaitNamedPipeW,
            },
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    },
    core::{BOOL, PCWSTR, PWSTR},
};

use super::{MAX_FRAME, relay_frames, upstream_candidates};

/// How long a connection waits for a busy upstream agent to free an instance.
const UPSTREAM_BUSY_WAIT_MS: u32 = 2_000;

/// How long to wait before retrying an instance the system refused to create.
const CREATE_RETRY: Duration = Duration::from_secs(1);

/// How long a handover from a daemon that could not pass an instance waits for
/// that daemon to let go of the name. It is exiting, so this is brief.
const LEGACY_RELEASE_WAIT: Duration = Duration::from_secs(5);

/// `ACCESS_ALLOWED_ACE_TYPE`, which lives in a `windows` module this crate does
/// not otherwise need.
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
/// `ACCESS_DENIED_ACE_TYPE`, likewise.
const ACCESS_DENIED_ACE_TYPE: u8 = 1;

struct PaneListener {
    name: PathBuf,
    stop: Arc<AtomicBool>,
}

/// The instance a listener starts from.
enum FirstInstance {
    /// Create the name, refusing one that already exists.
    Create,
    /// Create the name once a previous daemon from before instance handover
    /// has let go of it.
    CreateAfterRelease,
    /// An instance the previous daemon created and duplicated into this one.
    Adopted(usize),
}

static LISTENERS: LazyLock<Mutex<HashMap<u64, PaneListener>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Starts serving a fresh agent pipe for `pane_id`, and returns its name.
///
/// `fallback` is the agent the pane would have used without Zosh. Failure is
/// logged rather than returned, and leaves the pane without a daemon pipe: a
/// pane without agent forwarding still works.
pub(in crate::server) fn serve(pane_id: u64, fallback: Option<PathBuf>) -> Option<PathBuf> {
    let name = match crate::paths::new_pane_forwarded_agent_pipe(pane_id) {
        Ok(name) => name,
        Err(error) => {
            log::warn!("could not name pane {pane_id}'s agent pipe: {error:#}");
            return None;
        }
    };
    start(pane_id, name, FirstInstance::Create, fallback)
}

/// Serves `name` again for a pane an upgrade handed over.
///
/// `instance` is the handle value the previous daemon duplicated into this
/// process ([`hand_over`]); `None` from a daemon that predates that, whose own
/// instances have to go before this one can create the name.
pub(in crate::server) fn adopt(
    pane_id: u64,
    name: PathBuf,
    instance: Option<u64>,
    fallback: Option<PathBuf>,
) {
    let first = match instance.and_then(|value| usize::try_from(value).ok()) {
        Some(value) => FirstInstance::Adopted(value),
        None => FirstInstance::CreateAfterRelease,
    };
    start(pane_id, name, first, fallback);
}

fn start(
    pane_id: u64,
    name: PathBuf,
    first: FirstInstance,
    fallback: Option<PathBuf>,
) -> Option<PathBuf> {
    let mut listeners = LISTENERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(listener) = listeners.get(&pane_id) {
        return Some(listener.name.clone());
    }
    let wide_name = wide(&name);
    let first = user_only_descriptor().and_then(|descriptor| {
        let handle = first_instance(&wide_name, &descriptor, first)?;
        Ok((descriptor, handle.0 as usize))
    });
    let (descriptor, first) = match first {
        Ok(first) => first,
        Err(error) => {
            log::warn!(
                "could not serve pane {pane_id}'s agent pipe {}: {error}",
                name.display()
            );
            return None;
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    let spawned = thread::Builder::new()
        .name(format!("zmux-agent-pipe-{pane_id}"))
        .spawn({
            let stop = Arc::clone(&stop);
            move || accept_loop(pane_id, &wide_name, &descriptor, first, fallback, &stop)
        });
    if let Err(error) = spawned {
        // The closure, and the instance it owned, are gone with the failure.
        log::warn!("could not serve pane {pane_id}'s agent pipe: {error}");
        return None;
    }
    listeners.insert(
        pane_id,
        PaneListener {
            name: name.clone(),
            stop,
        },
    );
    Some(name)
}

fn first_instance(name: &[u16], descriptor: &str, first: FirstInstance) -> io::Result<HANDLE> {
    match first {
        FirstInstance::Create => create_instance(name, descriptor, true),
        FirstInstance::CreateAfterRelease => {
            let deadline = std::time::Instant::now() + LEGACY_RELEASE_WAIT;
            loop {
                match create_instance(name, descriptor, true) {
                    Ok(handle) => return Ok(handle),
                    Err(error) if std::time::Instant::now() >= deadline => return Err(error),
                    Err(_) => thread::sleep(Duration::from_millis(50)),
                }
            }
        }
        FirstInstance::Adopted(value) => {
            let handle = HANDLE(value as _);
            if let Err(error) = instance_is_private(handle) {
                let _ = unsafe { CloseHandle(handle) };
                return Err(error);
            }
            Ok(handle)
        }
    }
}

/// The pane's agent pipe name, if it is served, and a new instance of it
/// duplicated into `process`, for an upgrade's replacement to continue serving
/// from.
///
/// The duplicate keeps the name held after this process exits, so there is no
/// moment between the two daemons when another account could create it. If the
/// replacement never starts, the instance goes with it.
pub(in crate::server) fn hand_over(
    pane_id: u64,
    process: HANDLE,
) -> Option<(PathBuf, Option<u64>)> {
    let name = LISTENERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&pane_id)?
        .name
        .clone();
    let duplicated = user_only_descriptor().and_then(|descriptor| {
        let instance = create_instance(&wide(&name), &descriptor, false)?;
        let mut remote = HANDLE::default();
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                instance,
                process,
                &mut remote,
                0,
                false,
                DUPLICATE_SAME_ACCESS | DUPLICATE_CLOSE_SOURCE,
            )
        }
        .map_err(io::Error::other)?;
        Ok(remote.0 as usize as u64)
    });
    match duplicated {
        Ok(instance) => Some((name, Some(instance))),
        Err(error) => {
            // The replacement then waits for this daemon's instances to go and
            // creates the name itself, which is the gap this exists to close.
            log::warn!("could not hand over pane {pane_id}'s agent pipe instance: {error}");
            Some((name, None))
        }
    }
}

/// Closes an instance an upgrade handed over for a pane this daemon is not
/// adopting after all.
pub(in crate::server) fn discard(instance: Option<u64>) {
    if let Some(value) = instance.and_then(|value| usize::try_from(value).ok()) {
        let _ = unsafe { CloseHandle(HANDLE(value as _)) };
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
        // The listener always has an instance waiting, so an open from here
        // either wakes its blocked connect or is found by the next one.
        let _ = OpenOptions::new()
            .read(true)
            .write(true)
            .security_qos_flags(SECURITY_IDENTIFICATION.0)
            .open(&listener.name);
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
    name: &[u16],
    descriptor: &str,
    first: usize,
    fallback: Option<PathBuf>,
    stop: &AtomicBool,
) {
    let mut pending = HANDLE(first as _);
    loop {
        if stop.load(Ordering::Acquire) {
            let _ = unsafe { CloseHandle(pending) };
            return;
        }
        if let Err(error) = unsafe { ConnectNamedPipe(pending, None) } {
            let code = error.code().0 as u32 & 0xffff;
            if code == ERROR_NO_DATA.0 {
                // A client that came and went: the instance can listen again.
                let _ = unsafe { DisconnectNamedPipe(pending) };
                continue;
            }
            if code != ERROR_PIPE_CONNECTED.0 {
                log::warn!("pane {pane_id}'s agent pipe stopped listening: {error}");
                let _ = unsafe { CloseHandle(pending) };
                return;
            }
        }
        if stop.load(Ordering::Acquire) {
            let _ = unsafe { CloseHandle(pending) };
            return;
        }
        // Created while `pending` still exists, so the name is never without
        // an instance for another account to claim.
        let next = match create_instance(name, descriptor, false) {
            Ok(next) => next,
            Err(error) => {
                log::warn!("pane {pane_id}'s agent pipe could not add an instance: {error}");
                let _ = unsafe { DisconnectNamedPipe(pending) };
                thread::sleep(CREATE_RETRY);
                continue;
            }
        };
        let client = unsafe { File::from_raw_handle(pending.0 as _) };
        let fallback = fallback.clone();
        let _ = thread::Builder::new()
            .name(format!("zmux-agent-relay-{pane_id}"))
            .spawn(move || relay_connection(pane_id, client, fallback.as_deref()));
        pending = next;
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

/// Opens an upstream agent at identification level, so whatever serves that
/// name — a relay's pipe, a published path — cannot act as this daemon.
fn connect_upstream(path: &Path) -> io::Result<File> {
    let open = || {
        OpenOptions::new()
            .read(true)
            .write(true)
            .security_qos_flags(SECURITY_IDENTIFICATION.0)
            .open(path)
    };
    match open() {
        Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => {
            let wide = wide(path);
            let _ = unsafe { WaitNamedPipeW(PCWSTR(wide.as_ptr()), UPSTREAM_BUSY_WAIT_MS) };
            open()
        }
        result => result,
    }
}

fn create_instance(name: &[u16], descriptor: &str, first: bool) -> io::Result<HANDLE> {
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
    let open_mode = if first {
        PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE
    } else {
        PIPE_ACCESS_DUPLEX
    };
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            open_mode,
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

/// Checks that a pipe instance this process did not create is one only this
/// account can use: owned by this account — or by the token's default owner,
/// which for an elevated token is the Administrators group — and granting
/// access to nobody but this account and SYSTEM.
fn instance_is_private(handle: HANDLE) -> io::Result<()> {
    let user = token_information(TokenUser)?;
    let token_owner = token_information(TokenOwner)?;
    // SAFETY: both buffers were filled by GetTokenInformation for these
    // classes and are aligned for the structures read here.
    let (user_sid, owner_sid) = unsafe {
        (
            (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid,
            (*token_owner.as_ptr().cast::<TOKEN_OWNER>()).Owner,
        )
    };
    let mut owner = PSID::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut security = PSECURITY_DESCRIPTOR::default();
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut dacl),
            None,
            Some(&mut security),
        )
    };
    if status.is_err() {
        return Err(io::Error::from_raw_os_error(status.0 as i32));
    }
    let verdict = unsafe { descriptor_is_private(owner, dacl, user_sid, owner_sid) };
    unsafe { LocalFree(Some(HLOCAL(security.0))) };
    verdict
}

/// # Safety
///
/// `owner` and `dacl` must point into a live security descriptor, and the two
/// token SIDs must be valid.
unsafe fn descriptor_is_private(
    owner: PSID,
    dacl: *const ACL,
    user: PSID,
    token_owner: PSID,
) -> io::Result<()> {
    let refuse = |what: &str| {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the agent pipe instance {what}"),
        ))
    };
    unsafe {
        if owner.is_invalid()
            || (EqualSid(owner, user).is_err() && EqualSid(owner, token_owner).is_err())
        {
            return refuse("is owned by another account");
        }
        // A null DACL grants everyone everything.
        if dacl.is_null() {
            return refuse("has no DACL");
        }
        for index in 0..u32::from((*dacl).AceCount) {
            let mut ace = std::ptr::null_mut();
            GetAce(dacl, index, &mut ace).map_err(io::Error::other)?;
            let header = &*ace.cast::<windows::Win32::Security::ACE_HEADER>();
            match header.AceType {
                ACCESS_ALLOWED_ACE_TYPE => {}
                // A deny entry only narrows access.
                ACCESS_DENIED_ACE_TYPE => continue,
                _ => return refuse("carries an access entry this daemon never writes"),
            }
            let allowed = &*ace.cast::<windows::Win32::Security::ACCESS_ALLOWED_ACE>();
            let sid = PSID(std::ptr::addr_of!(allowed.SidStart).cast_mut().cast());
            if EqualSid(sid, user).is_err() && !IsWellKnownSid(sid, WinLocalSystemSid).as_bool() {
                return refuse("grants access to another account");
            }
        }
    }
    Ok(())
}

/// One of this process's token information classes, in a buffer aligned for
/// the structure it holds.
fn token_information(class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<u64>> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(io::Error::other)?;
        let mut length = 0_u32;
        let _ = GetTokenInformation(token, class, None, 0, &mut length);
        let mut buffer = vec![0_u64; (length as usize).div_ceil(8)];
        let queried = GetTokenInformation(
            token,
            class,
            Some(buffer.as_mut_ptr().cast()),
            length,
            &mut length,
        );
        let _ = CloseHandle(token);
        queried.map_err(io::Error::other)?;
        Ok(buffer)
    }
}

/// A DACL granting this account and SYSTEM, by the account's own SID.
///
/// Not `OW` (owner rights): an elevated token's default owner is the
/// Administrators group, so a pipe created from an elevated SSH login would
/// refuse the same account's non-elevated processes, and the reverse.
fn user_only_descriptor() -> io::Result<String> {
    let user = token_information(TokenUser)?;
    unsafe {
        let user = &*user.as_ptr().cast::<TOKEN_USER>();
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

#[cfg(test)]
#[path = "../../tests/server/agent_pipe/listener.rs"]
mod tests;
