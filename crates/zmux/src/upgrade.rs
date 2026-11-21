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
//! On platforms whose local-socket credentials survive `execv`, the listening
//! socket survives as an inherited descriptor, so clients never see a rebind
//! window while the daemon is replaced. macOS rebinds the listener in the new
//! image because inheriting it loses the peer credentials used to authorize
//! requests.
//!
//! Two rules follow from `execv` being irreversible, and both are enforced
//! here. The replacement is checked before it is run, so a daemon that cannot
//! be replaced keeps running rather than taking its sessions down with it. And
//! the image is the one this daemon resolved at startup, never a path a client
//! asked for: anything that could choose the replacement could inherit the
//! terminals of every protected session it holds.

use std::{
    ffi::CString,
    os::fd::{AsRawFd as _, BorrowedFd, FromRawFd as _, OwnedFd, RawFd},
};

use anyhow::{Context as _, Result};

#[path = "upgrade_handover.rs"]
mod handover;
pub use handover::{
    AttachmentHandover, HANDOVER_VERSION, Handover, PaneHandover, SessionHandover,
    SharedClientHandover, accepts_handover_version,
};

/// Descriptors made inheritable for the next image, made close-on-exec again
/// when this is dropped — which, for an `execv` that succeeds, it never is.
///
/// Cleared as late as possible and all at once, because every process the
/// daemon starts while one is clear inherits it. A carried pane master in a
/// shell lets that shell read and type into a session it has no secret for,
/// and a carried listener lets it accept connections meant for the daemon.
#[must_use = "the descriptors are inheritable only while this is held"]
pub struct InheritedAcrossExec<'a> {
    descriptors: Vec<BorrowedFd<'a>>,
}

impl<'a> InheritedAcrossExec<'a> {
    /// Clears close-on-exec on every one of `descriptors`, or on none of them.
    ///
    /// Everything the daemon intends to keep has to be cleared explicitly: a
    /// descriptor that stays close-on-exec is silently gone after the `execv`,
    /// and the first sign of it would be a session whose terminal no longer
    /// responds.
    pub fn clear(descriptors: impl IntoIterator<Item = BorrowedFd<'a>>) -> Result<Self> {
        let mut inherited = Self {
            descriptors: Vec::new(),
        };
        for descriptor in descriptors {
            set_close_on_exec(descriptor, false)?;
            inherited.descriptors.push(descriptor);
        }
        Ok(inherited)
    }
}

impl Drop for InheritedAcrossExec<'_> {
    fn drop(&mut self) {
        for descriptor in &self.descriptors {
            if let Err(error) = set_close_on_exec(*descriptor, true) {
                log::error!(
                    "descriptor {} is left inheritable after a failed upgrade: {error:#}",
                    descriptor.as_raw_fd()
                );
            }
        }
    }
}

/// Sets or clears a descriptor's close-on-exec flag.
pub fn set_close_on_exec(descriptor: BorrowedFd<'_>, close: bool) -> Result<()> {
    let raw = descriptor.as_raw_fd();
    // SAFETY: the borrow keeps the descriptor open for the call.
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFD) };
    anyhow::ensure!(flags >= 0, "reading descriptor flags: {}", last_error());
    let flags = if close {
        flags | libc::FD_CLOEXEC
    } else {
        flags & !libc::FD_CLOEXEC
    };
    // SAFETY: as above, with a flag set derived from the one just read.
    let result = unsafe { libc::fcntl(raw, libc::F_SETFD, flags) };
    anyhow::ensure!(result >= 0, "setting close-on-exec: {}", last_error());
    Ok(())
}

/// Takes ownership of a descriptor the previous image left open across the
/// `execv`, and makes it close-on-exec again before anything can inherit it.
///
/// The previous image had to clear the flag for the descriptor to survive, and
/// nothing on the spawn path closes strays: a descriptor claimed without
/// setting it back reaches every shell this image starts.
///
/// # Safety
///
/// Nothing else in this process may own `descriptor`, and it must be claimed
/// at most once.
pub unsafe fn claim_inherited_fd(descriptor: RawFd) -> Result<OwnedFd> {
    // Checked first, because taking ownership of a descriptor that is not open
    // aborts the process on close.
    anyhow::ensure!(
        descriptor_is_open(descriptor),
        "descriptor {descriptor} was not inherited"
    );
    // SAFETY: open, as just checked; the caller guarantees nothing else owns
    // it, and the borrow ends before ownership is taken.
    set_close_on_exec(unsafe { BorrowedFd::borrow_raw(descriptor) }, true)?;
    // SAFETY: as above.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

/// Marks every descriptor above standard error close-on-exec.
///
/// Run when the daemon starts, before it claims anything. Marking does not
/// close, so descriptors a previous image carried can still be claimed; what it
/// stops is anything left unclaimed — or inherited from whatever started the
/// daemon — reaching the shells it starts. Best effort: a descriptor this
/// misses is one the daemon was already going to leak.
pub fn mark_inherited_close_on_exec() {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: `close_range` with `CLOSE_RANGE_CLOEXEC` only sets a flag on
        // whichever descriptors in the range are open. Called through
        // `syscall` because not every libc this builds against wraps it.
        let marked = unsafe {
            libc::syscall(
                libc::SYS_close_range,
                3 as libc::c_uint,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            )
        };
        if marked == 0 {
            return;
        }
        // Kernels before 5.11 do not know the flag; sweep instead.
    }
    let descriptors = match std::fs::read_dir("/dev/fd") {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<RawFd>().ok())
            .filter(|descriptor| *descriptor > 2)
            .collect::<Vec<_>>(),
        Err(error) => {
            log::warn!("could not list this process's descriptors: {error}");
            return;
        }
    };
    // The listing's own descriptor is among these and closed by now, which
    // the check skips.
    for descriptor in descriptors {
        if descriptor_is_open(descriptor) {
            // SAFETY: open, as just checked, and borrowed only for the call.
            let _ = set_close_on_exec(unsafe { BorrowedFd::borrow_raw(descriptor) }, true);
        }
    }
}

fn last_error() -> std::io::Error {
    std::io::Error::last_os_error()
}

/// Writes the handover into an anonymous file and returns its descriptor.
///
/// Anonymous because it carries session verifiers: a named path would put them
/// on the filesystem, however briefly, where the security model says they never
/// go. The descriptor is inherited by the next image and named in its argv, but
/// stays close-on-exec until the `execv` itself (see [`InheritedAcrossExec`]):
/// anything else started from this process in between would inherit every
/// session's verifier.
pub fn write_handover(handover: &Handover) -> Result<std::fs::File> {
    let encoded = serde_json::to_vec(handover).context("serializing the handover")?;
    let mut file = tempfile_anonymous()?;
    use std::io::{Seek as _, Write as _};
    file.write_all(&encoded).context("writing the handover")?;
    file.rewind().context("rewinding the handover")?;
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
    // SAFETY: the descriptor was inherited from the previous image, which
    // opened it, and this takes ownership of it exactly once.
    let mut file = std::fs::File::from(
        unsafe { claim_inherited_fd(descriptor) }.context("claiming the handover")?,
    );
    let mut encoded = Vec::new();
    file.read_to_end(&mut encoded)
        .context("reading the handover")?;
    let handover: Handover = serde_json::from_slice(&encoded).context("parsing the handover")?;
    anyhow::ensure!(
        accepts_handover_version(handover.version),
        "handover version {} is not the {HANDOVER_VERSION} this multiplexer understands",
        handover.version
    );
    Ok(handover)
}

#[cfg(target_os = "linux")]
fn tempfile_anonymous() -> Result<std::fs::File> {
    let name = c"zmux-handover";
    // SAFETY: the name is a valid C string and the flags are a valid set.
    let raw = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    anyhow::ensure!(raw >= 0, "creating the handover: {}", last_error());
    // SAFETY: memfd_create returned a fresh descriptor this owns.
    Ok(unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(raw) })
}

#[cfg(not(target_os = "linux"))]
fn tempfile_anonymous() -> Result<std::fs::File> {
    // Unlinked immediately, so it has no name for anything else to open even
    // while it exists. Opened close-on-exec, as `std` opens everything.
    tempfile::tempfile().context("creating the handover")
}

/// Checks that `executable` can take over before anything irreversible happens.
///
/// Runs the candidate as a subprocess and asks whether it understands this
/// handover version. A daemon that skipped this and executed a replacement
/// which then refused the handover would have destroyed its own sessions to
/// find out.
pub fn replacement_accepts_handover(executable: &std::path::Path) -> Result<bool> {
    let mut command = std::process::Command::new(executable);
    #[cfg(target_os = "macos")]
    if crate::macos_application::is_application(executable) {
        command.arg("mux");
    }
    let status = command
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
    listener: Option<RawFd>,
) -> Result<std::convert::Infallible> {
    let program = CString::new(executable.as_os_str().as_encoded_bytes())
        .context("the multiplexer's own path is not a valid C string")?;
    let resume = CString::new(format!("--resume-from={handover}"))?;
    let daemon = CString::new("--daemon")?;
    let resume_listener = listener
        .map(|listener| CString::new(format!("--resume-listener={listener}")))
        .transpose()?;
    let mut arguments = vec![program.as_ptr()];
    #[cfg(target_os = "macos")]
    if crate::macos_application::is_application(executable) {
        arguments.push(c"mux".as_ptr());
    }
    arguments.extend([daemon.as_ptr(), resume.as_ptr()]);
    if let Some(resume_listener) = resume_listener.as_ref() {
        arguments.push(resume_listener.as_ptr());
    }
    arguments.push(std::ptr::null());
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
