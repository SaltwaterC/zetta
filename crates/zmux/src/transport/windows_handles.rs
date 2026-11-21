//! The Windows half of the transport. There is no `SCM_RIGHTS`, so a
//! terminal's handles are duplicated into the receiving process, after its
//! user has been checked and it has answered a challenge proving it is the
//! process it claims to be.

use super::*;

pub fn peer_process_id(_stream: &Stream) -> Result<Option<u32>> {
    Ok(None)
}

pub fn peer_is_this_user(_stream: &Stream) -> Result<bool> {
    Ok(true)
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
    use std::os::windows::io::AsRawHandle as _;
    use windows::Win32::Foundation::{
        CloseHandle, DUPLICATE_HANDLE_OPTIONS, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: both access rights are valid for `OpenProcess`, and the returned
    // handle is closed below on every path.
    let target = unsafe {
        OpenProcess(
            PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            process_id,
        )
    }
    .with_context(|| format!("opening process {process_id} to hand over a terminal"))?;

    let duplicated = (|| -> Result<Vec<i64>> {
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
    })();

    // SAFETY: `target` came from `OpenProcess` and is closed exactly once.
    unsafe {
        let _ = CloseHandle(target);
    }
    duplicated
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
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows::Win32::System::Threading::OpenProcessToken;

    let mut token = windows::Win32::Foundation::HANDLE::default();
    // SAFETY: `process` is open with at least query access, and `token` is a
    // valid writable slot closed below.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }
        .context("opening a process token")?;

    let sid = (|| -> Result<Vec<u8>> {
        let mut needed = 0;
        // SAFETY: asking for the required size with a null buffer is the
        // documented first step and is expected to fail.
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
        anyhow::ensure!(needed > 0, "a process token reported no user information");
        let mut buffer = vec![0u8; needed as usize];
        // SAFETY: the buffer is exactly the size the call just asked for.
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                needed,
                &mut needed,
            )
        }
        .context("reading a process token's user")?;

        // SAFETY: the buffer holds a `TOKEN_USER` whose `Sid` points inside it.
        let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
        // SAFETY: the SID is valid for as long as the buffer is.
        let length = unsafe { windows::Win32::Security::GetLengthSid(user.User.Sid) } as usize;
        anyhow::ensure!(length > 0, "a process token reported an empty user");
        // SAFETY: `length` is the SID's own reported size.
        Ok(unsafe { std::slice::from_raw_parts(user.User.Sid.0 as *const u8, length) }.to_vec())
    })();

    // SAFETY: `token` came from `OpenProcessToken` and is closed exactly once.
    unsafe {
        let _ = CloseHandle(token);
    }
    sid
}

/// Proof that the peer of a connection really is the process it named.
///
/// Windows has no peer credentials for `AF_UNIX`, and nothing the peer *sends*
/// can establish its identity: any same-user process may open another and
/// duplicate that one's handles, so a handle arriving from the peer shows only
/// that the peer could open something. What only the named process can do is
/// read a handle that exists in its own table. So the daemon writes a nonce
/// into a pipe, duplicates the read end into the process the peer claims to be,
/// and asks for the nonce back. An impostor is handed a handle that is valid
/// only inside its victim, and cannot read it.
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
        use std::os::windows::io::{AsHandle as _, FromRawHandle as _, OwnedHandle};
        use windows::Win32::{Foundation::HANDLE, System::Pipes::CreatePipe};

        let mut read = HANDLE::default();
        let mut write = HANDLE::default();
        // SAFETY: both destinations are valid writable slots, and a default
        // buffer size is what an anonymous pipe carrying one nonce needs.
        unsafe { CreatePipe(&mut read, &mut write, None, 0) }
            .context("creating a peer attestation pipe")?;
        // SAFETY: `CreatePipe` succeeded, so each handle is owned here exactly
        // once and closed when its wrapper drops.
        let read = unsafe { OwnedHandle::from_raw_handle(read.0.cast()) };
        // SAFETY: as above, for the other end.
        let write = unsafe { OwnedHandle::from_raw_handle(write.0.cast()) };

        let nonce = random_hex(Self::NONCE_BYTES)?;
        {
            // Dropped before the handle is handed over, so the reader sees the
            // end of the nonce instead of waiting for a writer that never
            // writes again.
            let mut writer = std::fs::File::from(write);
            writer
                .write_all(nonce.as_bytes())
                .context("writing a peer attestation nonce")?;
        }

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
    use windows::Win32::Foundation::{
        CloseHandle, DUPLICATE_CLOSE_SOURCE, DUPLICATE_HANDLE_OPTIONS, DuplicateHandle, HANDLE,
    };
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_DUP_HANDLE};

    // SAFETY: the access right is valid for `OpenProcess`, and the returned
    // handle is closed below on every path.
    let target = unsafe { OpenProcess(PROCESS_DUP_HANDLE, false, process_id) }
        .with_context(|| format!("opening process {process_id} to close a handle in it"))?;
    // SAFETY: `target` is open with duplication access; a null destination
    // process with `DUPLICATE_CLOSE_SOURCE` is how a handle is closed in
    // another process without duplicating it anywhere.
    let closed = unsafe {
        DuplicateHandle(
            target,
            HANDLE(handle as *mut _),
            HANDLE::default(),
            std::ptr::null_mut(),
            0,
            false,
            DUPLICATE_HANDLE_OPTIONS(DUPLICATE_CLOSE_SOURCE.0),
        )
    };
    // SAFETY: `target` came from `OpenProcess` and is closed exactly once.
    unsafe {
        let _ = CloseHandle(target);
    }
    closed.context("closing a handle in the client")
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
