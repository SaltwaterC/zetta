//! The Windows half of the transport: asking the kernel which process is on
//! the far end of a socket, and, because there is no `SCM_RIGHTS`, duplicating
//! a terminal's handles into the receiving process after its user has been
//! checked.

use super::*;

/// The process on the far end of an accepted connection, once it has been
/// confirmed to run as this user.
///
/// The kernel records which process connected an `AF_UNIX` socket and answers
/// `SIO_AF_UNIX_GETPEERPID` with it, so this is as good as `SO_PEERCRED` on
/// Linux: nothing the peer sends affects it. The process is opened to compare
/// its token's user with this one's, and the handle is kept for as long as this
/// value lives, so the id cannot pass to another process while the request it
/// identifies is being served.
///
/// A Windows release that does not answer the query leaves the id `None`; the
/// daemon then asks for a [`PeerChallenge`] where it needs an identity.
pub struct PeerProcess {
    id: Option<u32>,
    _process: Option<std::os::windows::io::OwnedHandle>,
}

impl PeerProcess {
    /// Refuses a peer running as another user.
    pub fn of(stream: &Stream) -> Result<Self> {
        use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

        let Some(id) = reported_peer_process_id(stream)? else {
            return Ok(Self {
                id: None,
                _process: None,
            });
        };
        // SAFETY: the access right is valid for `OpenProcess`.
        let opened = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, id) }
            .with_context(|| format!("opening the connecting process {id}"))?;
        // SAFETY: `OpenProcess` succeeded, so this function owns the handle.
        let process = unsafe { OwnedHandle::from_raw_handle(opened.0) };
        anyhow::ensure!(
            process_belongs_to_this_user(HANDLE(process.as_raw_handle()))?,
            "refusing a connection from another user"
        );
        Ok(Self {
            id: Some(id),
            _process: Some(process),
        })
    }

    /// The peer's process id, where the kernel reported one.
    pub fn id(&self) -> Option<u32> {
        self.id
    }
}

/// The process that connected `stream`, as the kernel recorded it, or `None`
/// where this Windows release does not answer the query.
fn reported_peer_process_id(stream: &Stream) -> Result<Option<u32>> {
    use std::os::windows::io::AsRawSocket as _;
    use windows::Win32::Networking::WinSock::{
        SIO_AF_UNIX_GETPEERPID, SOCKET, WSAEINVAL, WSAEOPNOTSUPP, WSAGetLastError, WSAIoctl,
    };

    let mut process_id = 0_u32;
    let mut written = 0_u32;
    // SAFETY: `stream` keeps the socket open for the call. The output buffer is
    // `process_id`, whose size is passed alongside it, and with neither an
    // overlapped structure nor a completion routine the call completes before
    // it returns, so nothing writes to either local afterwards.
    let status = unsafe {
        WSAIoctl(
            SOCKET(stream.as_raw_socket() as usize),
            SIO_AF_UNIX_GETPEERPID,
            None,
            0,
            Some((&raw mut process_id).cast()),
            size_of::<u32>() as u32,
            &mut written,
            None,
            None,
        )
    };
    if status != 0 {
        // SAFETY: takes no arguments and reads this thread's last socket error.
        let error = unsafe { WSAGetLastError() };
        if error == WSAEOPNOTSUPP || error == WSAEINVAL {
            return Ok(None);
        }
        return Err(std::io::Error::from_raw_os_error(error.0))
            .context("asking which process connected");
    }
    // Windows writes the id but leaves `written` at 0 (seen on build 26300),
    // so the id itself is the only sign of an answer; it was zeroed above, and
    // no process has id 0.
    anyhow::ensure!(process_id != 0, "the kernel reported no connecting process");
    Ok(Some(process_id))
}

/// Sends a terminal's handles to the client that asked for it.
///
/// Windows has no `SCM_RIGHTS`: a handle is moved by duplicating it into the
/// target process, which means the sender needs a handle *to that process*.
/// The client therefore names itself in its request, and the duplicated values
/// travel in the reply as plain integers.
///
/// The peer's user is checked first. On Unix the kernel answers that question
/// about the connected socket; here the process is opened and its token
/// compared, because the process identifier came from the client and a handle
/// to a session's terminal must not cross a user boundary on the strength of
/// something the other side claimed.
pub fn duplicate_to(
    process_id: u32,
    handles: &[std::os::windows::io::BorrowedHandle<'_>],
) -> Result<Vec<i64>> {
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use windows::Win32::Foundation::{
        DUPLICATE_HANDLE_OPTIONS, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: both access rights are valid for `OpenProcess`.
    let opened = unsafe {
        OpenProcess(
            PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            process_id,
        )
    }
    .with_context(|| format!("opening process {process_id} to hand over a terminal"))?;
    // SAFETY: `OpenProcess` succeeded, so this function owns the handle.
    let owned = unsafe { OwnedHandle::from_raw_handle(opened.0) };
    let target = HANDLE(owned.as_raw_handle());

    {
        anyhow::ensure!(
            process_belongs_to_this_user(target)?,
            "refusing to hand a terminal to process {process_id}, which belongs to another user"
        );
        let mut duplicated = Vec::with_capacity(handles.len());
        for handle in handles {
            let mut target_handle = HANDLE::default();
            // SAFETY: the source handle is owned by this process for the
            // duration, and the destination is a valid writable slot.
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    HANDLE(handle.as_raw_handle()),
                    target,
                    &mut target_handle,
                    0,
                    false,
                    DUPLICATE_HANDLE_OPTIONS(DUPLICATE_SAME_ACCESS.0),
                )
            }
            .context("duplicating a terminal handle into the client")?;
            duplicated.push(target_handle.0 as i64);
        }
        Ok(duplicated)
    }
}

/// Whether `process` runs as the same user as this one.
pub(super) fn process_belongs_to_this_user(
    process: windows::Win32::Foundation::HANDLE,
) -> Result<bool> {
    let theirs = user_sid(process)?;
    // SAFETY: a pseudo-handle for the current process, which must not be closed.
    let ours = user_sid(unsafe { windows::Win32::System::Threading::GetCurrentProcess() })?;
    Ok(theirs == ours)
}

/// The user SID of a process's token, as raw bytes for comparison.
pub(super) fn user_sid(process: windows::Win32::Foundation::HANDLE) -> Result<Vec<u8>> {
    let user = crate::private_fs::windows_impl::token_user(process)
        .context("reading a process token's user")?;
    let sid = user.sid();
    // SAFETY: the SID points into `user`, alive for the call.
    let length = unsafe { windows::Win32::Security::GetLengthSid(sid) } as usize;
    anyhow::ensure!(length > 0, "a process token reported an empty user");
    // SAFETY: `length` is the SID's own reported size, within `user`.
    Ok(unsafe { std::slice::from_raw_parts(sid.0.cast::<u8>(), length) }.to_vec())
}

/// Proof that the peer of a connection really is the process it named, for
/// a Windows release that does not report the peer itself (see
/// [`PeerProcess`]).
///
/// Without that report there are no peer credentials for `AF_UNIX`, and
/// nothing the peer *sends* can establish its identity: any same-user process
/// may open another and duplicate that one's handles, so a handle arriving
/// from the peer shows only that the peer could open something. What only the named process can do is
/// read a handle that exists in its own table. So the daemon writes a nonce
/// into a pipe, duplicates the read end into the process the peer claims to be,
/// and asks for the nonce back. An impostor is handed a handle that is valid
/// only inside its victim.
///
/// That stops an impostor that cannot open its victim, not one that can: a
/// same-user process of the same integrity can open the victim with
/// `PROCESS_DUP_HANDLE`, duplicate the handle out and read the nonce. Where the
/// kernel reports the peer, that impostor is named by the kernel instead and
/// gets nowhere; here, it is only kept from being taken at its word.
///
/// The handle is closed again inside the target when this drops, answered or
/// not, so naming somebody else's process repeatedly cannot fill that
/// process's handle table.
pub struct PeerChallenge {
    pub(super) process_id: u32,
    pub(super) handle: i64,
    pub(super) nonce: String,
}

impl PeerChallenge {
    /// Bytes of randomness behind the nonce, which travels as hex.
    pub const NONCE_BYTES: usize = 32;

    pub fn issue(process_id: u32) -> Result<Self> {
        use std::os::windows::io::AsHandle as _;

        let (read, mut writer) = std::io::pipe().context("creating a peer attestation pipe")?;

        let nonce = random_hex(Self::NONCE_BYTES)?;
        writer
            .write_all(nonce.as_bytes())
            .context("writing a peer attestation nonce")?;
        // Dropped before the handle is handed over, so the reader sees the end
        // of the nonce instead of waiting for a writer that never writes again.
        drop(writer);

        // Duplicating is also where the peer's user is checked: this refuses a
        // target belonging to another user.
        let handle = *duplicate_to(process_id, &[read.as_handle()])?
            .first()
            .context("no attestation handle was duplicated")?;
        Ok(Self {
            process_id,
            handle,
            nonce,
        })
    }

    /// The handle value, meaningful only inside the process it was issued to.
    pub fn handle(&self) -> i64 {
        self.handle
    }

    /// Constant-time, like every other secret comparison here.
    pub fn matches(&self, answer: &str) -> bool {
        token_matches(answer, &self.nonce)
    }
}

impl Drop for PeerChallenge {
    fn drop(&mut self) {
        if let Err(error) = close_in_process(self.process_id, self.handle) {
            log::debug!(
                "could not close attestation handle in process {}: {error:#}",
                self.process_id
            );
        }
    }
}

/// Closes a handle that lives in another process, which `DuplicateHandle` does
/// when it is asked to duplicate *from* that process into nowhere.
pub(super) fn close_in_process(process_id: u32, handle: i64) -> Result<()> {
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use windows::Win32::Foundation::{
        DUPLICATE_CLOSE_SOURCE, DUPLICATE_HANDLE_OPTIONS, DuplicateHandle, HANDLE,
    };
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_DUP_HANDLE};

    // SAFETY: the access right is valid for `OpenProcess`.
    let opened = unsafe { OpenProcess(PROCESS_DUP_HANDLE, false, process_id) }
        .with_context(|| format!("opening process {process_id} to close a handle in it"))?;
    // SAFETY: `OpenProcess` succeeded, so this function owns the handle.
    let target = unsafe { OwnedHandle::from_raw_handle(opened.0) };
    // SAFETY: `target` is open with duplication access; a null destination
    // process with `DUPLICATE_CLOSE_SOURCE` is how a handle is closed in
    // another process without duplicating it anywhere.
    unsafe {
        DuplicateHandle(
            HANDLE(target.as_raw_handle()),
            HANDLE(handle as *mut _),
            HANDLE::default(),
            std::ptr::null_mut(),
            0,
            false,
            DUPLICATE_HANDLE_OPTIONS(DUPLICATE_CLOSE_SOURCE.0),
        )
    }
    .context("closing a handle in the client")
}

/// Answers a [`PeerChallenge`] by reading the nonce the multiplexer duplicated
/// into this process. Only the process it was issued to can do this.
pub fn answer_challenge(handle: i64) -> Result<String> {
    use std::io::Read as _;
    use std::os::windows::io::BorrowedHandle;

    // The daemon owns this handle in our process and closes it when the
    // challenge ends. Closing it here would let Windows reuse its value before
    // the daemon's cleanup, which could then close an unrelated handle.
    // SAFETY: the daemon duplicated this handle into this process and keeps
    // the challenge alive while waiting for the answer.
    let handle = unsafe { BorrowedHandle::borrow_raw(handle as *mut _) };
    let mut answer = String::new();
    std::fs::File::from(handle.try_clone_to_owned()?)
        .take((PeerChallenge::NONCE_BYTES * 2) as u64)
        .read_to_string(&mut answer)
        .context("reading the multiplexer's attestation nonce")?;
    Ok(answer)
}

/// Claims handles the multiplexer duplicated into this process.
pub fn claim_duplicated(values: &[i64]) -> Vec<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::FromRawHandle as _;
    values
        .iter()
        .map(|value| {
            // SAFETY: the multiplexer duplicated this handle into this process
            // and does not retain the duplicate, so ownership is taken once.
            unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(*value as *mut _) }
        })
        .collect()
}
