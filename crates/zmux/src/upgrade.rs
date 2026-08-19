//! Replacing the daemon without ending the sessions it holds.
//!
//! The daemon re-executes *itself*: same process, new image. That is what
//! keeps the sessions alive — the PTY descriptors survive an `execv` once their
//! close-on-exec flag is cleared, and, critically, so does the parent/child
//! relationship with every shell. Handing the descriptors to a *new* process
//! would preserve the terminals but orphan the children, and their exit
//! statuses would be lost forever, because only a parent may reap. The same fact
//! decides how the replacement rebuilds each pane: it reaps its own children, so
//! it must never treat one as though it belonged to somebody else.
//!
//! The listening socket does *not* survive; it is rebound. See [`Handover`].
//!
//! Two rules follow from `execv` being irreversible, and both are enforced
//! here. The replacement is checked before it is run, so a daemon that cannot
//! be replaced keeps running rather than taking its sessions down with it. And
//! the image is the one this daemon resolved at startup, never a path a client
//! asked for: anything that could choose the replacement could inherit the
//! terminals of every protected session it holds.

use std::{
    ffi::CString,
    os::fd::{AsRawFd, RawFd},
    time::Duration,
};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

/// Bumped whenever [`Handover`] changes shape. The replacement refuses a
/// version it does not know rather than guessing at the layout of a session it
/// is about to take responsibility for.
pub const HANDOVER_VERSION: u32 = 5;

/// Everything the next image needs to carry on.
///
/// The listening socket is deliberately absent. It is not carried: `std` opens
/// sockets close-on-exec, so the replacement unlinks the path and binds it
/// again, which leaves a brief window in which a connection is refused. Clients
/// retry, and the endpoint token is preserved so the retry is accepted. Carrying
/// the socket instead would remove the window, but the descriptors that matter
/// are the terminals — losing the listener costs a reconnect, losing a terminal
/// costs the session.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handover {
    pub version: u32,
    pub next_session_id: u64,
    pub next_pane_id: u64,
    pub sessions: Vec<SessionHandover>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionHandover {
    pub id: u64,
    pub summary: crate::protocol::BackgroundSessionSummary,
    pub state: serde_json::Value,
    /// Whether the user asked for this session to outlive its window.
    pub keep: bool,
    /// Whether the user asked for this session to be joinable while a window is
    /// still showing it. Carried for the same reason `keep` is: dropping it would
    /// make `--upgrade` silently withdraw every shared session, and the windows
    /// showing them would have no way to notice.
    pub offered: bool,
    /// The process a session that is not offered belongs to. Carried for the
    /// same reason again: losing it would publish every backgrounded session to
    /// every window, which is the opposite of what backgrounding one means.
    #[serde(default)]
    pub owner: Option<u32>,
    /// The Argon2id verifier, so a protected session stays protected across an
    /// upgrade.
    pub verifier: Option<String>,
    /// Consecutive wrong secrets, and what is left of the window they opened.
    ///
    /// Carried rather than reset: dropping them would make `--upgrade` a way to
    /// clear a session's rate limit, which is exactly the thing the backoff
    /// exists to prevent. Stored as a remaining duration because the deadline
    /// is a monotonic instant with no meaning in another image.
    pub failed_authentications: u32,
    pub refuse_for: Option<Duration>,
    pub panes: Vec<PaneHandover>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneHandover {
    pub id: u64,
    /// The PTY master, which survives the `execv` and is reclaimed by number.
    ///
    /// No child-event socket travels with it. The daemon is the child's parent
    /// on both sides of an `execv` — that is the entire reason the upgrade is a
    /// re-exec — so the replacement reaps with `waitpid` and registers a fresh
    /// `SIGCHLD` pipe. Carrying the old pipe instead handed the new image a
    /// descriptor whose writer the exec had already closed, which read as end of
    /// file and so as "the process ended", falsely, for every pane that had been
    /// attached.
    pub descriptor: RawFd,
    pub child_pid: u32,
    /// Who held this pane, so the replacement offers it in the same mode. A
    /// client that dies across the upgrade is then still noticed by the
    /// liveness reclaim, and a pane mid-handover can still complete it.
    pub attachment: AttachmentHandover,
    /// The size the pane is running at, so arbitration continues from what
    /// every viewer is showing rather than restarting from a default.
    pub columns: u16,
    pub lines: u16,
    pub exited: bool,
    /// The status observed if it has already ended, so a client asking what it
    /// missed gets the real exit rather than "status unavailable".
    pub exit_status: Option<i32>,
    pub retained: Vec<u8>,
}

/// A pane's attachment, in the form that survives an exec.
///
/// Connections cannot be carried, so a shared pane keeps only *that* it is
/// shared and by whom. Collapsing anything other than an exclusive hold to "no
/// holder" — as this once did — was wrong twice over: a pane mid-handover became
/// readable by the daemon while its holder still had the descriptor and could no
/// longer complete the handover, and a shared pane came back exclusive-capable,
/// so a reconnecting viewer was handed the descriptor instead of rejoining the
/// relay.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "attachment", rename_all = "snake_case")]
pub enum AttachmentHandover {
    None,
    Exclusive { holder: u32 },
    Revoking { holder: u32 },
    Shared { clients: Vec<SharedClientHandover> },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedClientHandover {
    pub process_id: u32,
    pub columns: u16,
    pub lines: u16,
    /// Kept because a pane's exit reports which viewers typed into it, and an
    /// upgrade must not launder that away.
    pub input_sent: bool,
}

/// Clears close-on-exec so a descriptor survives into the next image.
///
/// Everything the daemon intends to keep has to be cleared explicitly: a
/// descriptor that stays close-on-exec is silently gone after the `execv`, and
/// the first sign of it would be a session whose terminal no longer responds.
pub fn keep_across_exec(descriptor: &impl AsRawFd) -> Result<()> {
    let raw = descriptor.as_raw_fd();
    // SAFETY: the descriptor is owned by the caller and open for the duration.
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFD) };
    anyhow::ensure!(flags >= 0, "reading descriptor flags: {}", last_error());
    // SAFETY: as above; the flag set is the one just read.
    let result = unsafe { libc::fcntl(raw, libc::F_SETFD, flags & !libc::FD_CLOEXEC) };
    anyhow::ensure!(result >= 0, "clearing close-on-exec: {}", last_error());
    Ok(())
}

fn last_error() -> std::io::Error {
    std::io::Error::last_os_error()
}

/// Writes the handover into an anonymous file and returns its descriptor.
///
/// Anonymous because it carries session verifiers: a named path would put them
/// on the filesystem, however briefly, where the security model says they never
/// go. The descriptor is inherited by the next image and named in its argv.
pub fn write_handover(handover: &Handover) -> Result<std::fs::File> {
    let encoded = serde_json::to_vec(handover).context("serializing the handover")?;
    let mut file = tempfile_anonymous()?;
    use std::io::{Seek as _, Write as _};
    file.write_all(&encoded).context("writing the handover")?;
    file.rewind().context("rewinding the handover")?;
    keep_across_exec(&file)?;
    Ok(file)
}

/// Whether a descriptor number is actually open in this process.
///
/// Taking ownership of one that is not aborts the process outright — Rust's
/// I/O safety check fires on the close — so a handover naming a descriptor
/// this image did not inherit has to be rejected before it is claimed.
pub fn descriptor_is_open(descriptor: RawFd) -> bool {
    // SAFETY: `F_GETFD` only reads the descriptor's flags and reports an error
    // for one that is not open.
    unsafe { libc::fcntl(descriptor, libc::F_GETFD) >= 0 }
}

pub fn read_handover(descriptor: RawFd) -> Result<Handover> {
    use std::io::Read as _;
    anyhow::ensure!(
        descriptor_is_open(descriptor),
        "the handover descriptor {descriptor} was not inherited"
    );
    // SAFETY: the descriptor was inherited from the previous image, which
    // opened it, and this takes ownership of it exactly once.
    let mut file = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(descriptor) };
    let mut encoded = Vec::new();
    file.read_to_end(&mut encoded)
        .context("reading the handover")?;
    let handover: Handover = serde_json::from_slice(&encoded).context("parsing the handover")?;
    anyhow::ensure!(
        handover.version == HANDOVER_VERSION,
        "handover version {} is not the {HANDOVER_VERSION} this multiplexer understands",
        handover.version
    );
    Ok(handover)
}

#[cfg(target_os = "linux")]
fn tempfile_anonymous() -> Result<std::fs::File> {
    let name = c"zmux-handover";
    // SAFETY: the name is a valid C string and the flags are a valid set.
    let raw = unsafe { libc::memfd_create(name.as_ptr(), 0) };
    anyhow::ensure!(raw >= 0, "creating the handover: {}", last_error());
    // SAFETY: memfd_create returned a fresh descriptor this owns.
    Ok(unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(raw) })
}

#[cfg(not(target_os = "linux"))]
fn tempfile_anonymous() -> Result<std::fs::File> {
    // Unlinked immediately, so it has no name for anything else to open even
    // while it exists.
    tempfile::tempfile().context("creating the handover")
}

/// Checks that `executable` can take over before anything irreversible happens.
///
/// Runs the candidate as a subprocess and asks whether it understands this
/// handover version. A daemon that skipped this and executed a replacement
/// which then refused the handover would have destroyed its own sessions to
/// find out.
pub fn replacement_accepts_handover(executable: &std::path::Path) -> Result<bool> {
    let status = std::process::Command::new(executable)
        .arg("--resume-check")
        .arg(HANDOVER_VERSION.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .with_context(|| format!("running {}", executable.display()))?;
    Ok(status.success())
}

/// Replaces this process with a fresh image of `executable`.
///
/// Never returns on success: the process is the same one, running new code,
/// still the parent of every session's shell.
pub fn exec_replacement(
    executable: &std::path::Path,
    handover: RawFd,
) -> Result<std::convert::Infallible> {
    let program = CString::new(executable.as_os_str().as_encoded_bytes())
        .context("the multiplexer's own path is not a valid C string")?;
    let resume = CString::new(format!("--resume-from={handover}"))?;
    let daemon = CString::new("--daemon")?;
    let arguments = [
        program.as_ptr(),
        daemon.as_ptr(),
        resume.as_ptr(),
        std::ptr::null(),
    ];
    // SAFETY: both pointers outlive the call, and the argument vector is
    // null-terminated as execv requires.
    unsafe {
        libc::execv(program.as_ptr(), arguments.as_ptr());
    }
    // execv only returns on failure.
    Err(last_error())
        .with_context(|| format!("replacing this multiplexer with {}", executable.display()))
}

#[cfg(test)]
#[path = "tests/upgrade.rs"]
mod tests;
