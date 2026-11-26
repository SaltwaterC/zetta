//! Replacing the Windows daemon while keeping its sessions.
//!
//! Windows cannot replace a running executable in place, and a pseudoconsole
//! belongs to the process that created it. The consoles therefore live in the
//! long-lived zmux-pty host; this module carries the daemon's ordinary session
//! state to its successor while the old daemon stops and the successor binds
//! the same endpoint.
//!
//! That state includes session verifiers, retained output and attachment
//! identities, so it never touches the disk. The old daemon writes it into an
//! anonymous pipe whose read end it duplicates into the replacement process
//! and closes in its own: the only handle that can read it is in the process
//! it was meant for. The replacement's stdin carries just that handle's value,
//! which means nothing in any other process. A failed upgrade leaves nothing
//! behind but a readiness marker, which says only "ready".
//!
//! What this does not change: on Windows a process running as the same user can
//! read another's memory, and so either daemon's copy of the state (see
//! `docs/background-sessions.md`). It closes the file, which any process could
//! read or rewrite without even that, and which outlived a failed upgrade.

use std::{
    fs::{self, File},
    io::{BufRead, Read as _, Write},
    os::windows::io::{AsRawHandle as _, FromRawHandle as _, IntoRawHandle as _},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use windows::Win32::{
    Foundation::{DUPLICATE_CLOSE_SOURCE, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE},
    Storage::FileSystem::{FILE_TYPE, FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileType},
    System::Threading::GetCurrentProcess,
};
use zeroize::Zeroizing;

#[path = "upgrade_handover.rs"]
mod handover;
pub use handover::{
    AttachmentHandover, HANDOVER_VERSION, Handover, PaneHandover, SessionHandover,
    SharedClientHandover, accepts_handover_version,
};

/// The largest handover a replacement reads. Retained output is bounded per
/// pane, so this only stops a broken sender from exhausting memory.
const MAX_HANDOVER_BYTES: u64 = 1 << 30;

/// Where a replacement reads its handover from.
#[derive(Debug)]
pub enum HandoverSource {
    /// A pipe handle the previous daemon duplicated into this process, whose
    /// value arrives on stdin (`--resume-from-pipe`).
    Pipe,
    /// A file written by a daemon from before [`HandoverSource::Pipe`]
    /// (`--resume-from=PATH`).
    File(PathBuf),
}

const READY_TIMEOUT: Duration = Duration::from_secs(10);
const READY_POLL: Duration = Duration::from_millis(10);

/// A fresh path for the replacement's readiness marker in the daemon's private
/// session directory. It holds nothing but the word "ready".
pub fn ready_path(directory: &Path) -> Result<PathBuf> {
    Ok(directory.join(format!(
        "zmux-handover-{}.ready",
        crate::transport::random_hex(16)?
    )))
}

pub fn remove_ready(ready: &Path) {
    let _ = fs::remove_file(ready);
}

/// Sends `handover` to `replacement`, started by [`spawn_replacement`], over a
/// pipe only that process can read.
///
/// The pipe's read end is duplicated into the replacement and closed here, and
/// its value written to the replacement's stdin; the state follows on the pipe
/// from a thread, since the replacement only drains it once it is running.
/// Should the replacement die or be killed, its end closes and the write fails,
/// so the thread never outlives a failed upgrade for long.
pub fn send_handover(
    replacement: &mut Child,
    handover: &Handover,
    lifetime_lock: Option<&File>,
) -> Result<()> {
    let stdin = replacement
        .stdin
        .take()
        .context("the Windows replacement has no stdin")?;
    send_handover_to(
        HANDLE(replacement.as_raw_handle()),
        stdin,
        handover,
        lifetime_lock,
    )
}

fn send_handover_to(
    process: HANDLE,
    mut announce: impl Write,
    handover: &Handover,
    lifetime_lock: Option<&File>,
) -> Result<()> {
    let encoded =
        Zeroizing::new(serde_json::to_vec(handover).context("serializing the Windows handover")?);
    // Not inheritable: the replacement is given the read end explicitly, and
    // nothing else this daemon starts may receive either end.
    let (read, mut writer) = std::io::pipe().context("creating the Windows handover pipe")?;
    let mut remote = HANDLE::default();
    // DUPLICATE_CLOSE_SOURCE closes this process's read end even on failure,
    // which leaves the replacement's copy the only one.
    // SAFETY: `read` is released from its owner here, and the call closes it,
    // so it is closed exactly once; `process` is a live process handle the
    // caller holds — the replacement's `Child`, or this process's pseudo
    // handle in the tests — and `remote` is writable storage.
    unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            HANDLE(read.into_raw_handle()),
            process,
            &mut remote,
            0,
            false,
            DUPLICATE_SAME_ACCESS | DUPLICATE_CLOSE_SOURCE,
        )
    }
    .context("passing the handover pipe to the Windows replacement")?;
    writeln!(announce, "{}", remote.0 as usize)
        .and_then(|()| announce.flush())
        .context("telling the Windows replacement where its handover is")?;
    if let Some(lock) = lifetime_lock {
        announce_lifetime_lock(process, &mut announce, lock);
    }
    drop(announce);
    std::thread::Builder::new()
        .name("zmux-handover".to_owned())
        .spawn(move || {
            if let Err(error) = writer.write_all(&encoded) {
                log::warn!("the Windows replacement did not take the whole handover: {error}");
            }
        })
        .context("starting the handover writer")?;
    Ok(())
}

/// Duplicates the lifetime lock into the replacement and announces it.
///
/// The source is kept: this daemon holds the lock until it exits, and the
/// replacement's copy holds it from then on, so it is never free in between.
/// Best effort — a replacement that is not given it takes the lock itself
/// once this daemon has exited.
fn announce_lifetime_lock(process: HANDLE, announce: &mut impl Write, lock: &File) {
    let mut remote = HANDLE::default();
    // SAFETY: both process handles are valid for the call, and the source is a
    // handle this process owns through `lock`.
    let duplicated = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            HANDLE(lock.as_raw_handle()),
            process,
            &mut remote,
            0,
            false,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if let Err(error) = duplicated
        .context("duplicating the multiplexer lock")
        .and_then(|()| {
            writeln!(announce, "{}", remote.0 as usize)
                .and_then(|()| announce.flush())
                .context("announcing the multiplexer lock")
        })
    {
        log::warn!("the Windows replacement will take the lock itself: {error:#}");
    }
}

/// Reads the handover the previous daemon sent to this process.
pub fn receive_handover(source: &HandoverSource) -> Result<Handover> {
    match source {
        HandoverSource::Pipe => receive_handover_from(std::io::stdin().lock()),
        HandoverSource::File(path) => {
            let encoded = fs::read(path)
                .map(Zeroizing::new)
                .with_context(|| format!("reading Windows handover {}", path.display()));
            // Gone as soon as it has been read, whatever happens next: the
            // daemon that wrote it is not going to read it again.
            let _ = fs::remove_file(path);
            parse_handover(&encoded?, HANDOVER_VERSION)
        }
    }
}

fn receive_handover_from(mut announce: impl BufRead) -> Result<Handover> {
    let mut line = String::new();
    announce
        .read_line(&mut line)
        .context("reading the handover pipe's handle")?;
    let value = line
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|value| *value != 0)
        .context("the previous daemon sent an unusable handover handle")?;
    // Checked before it is owned: a value that is not the pipe may be a handle
    // something else in this process holds, which owning would close.
    anyhow::ensure!(
        is_file_of_type(value, FILE_TYPE_PIPE),
        "the handover handle is not a pipe"
    );
    // SAFETY: the previous daemon duplicated this handle into this process
    // for it to own, nothing else here knows its value, and it is the pipe.
    let pipe = unsafe { File::from_raw_handle(value as _) };
    let lifetime_lock = receive_lifetime_lock(&mut announce);
    let mut encoded = Zeroizing::new(Vec::new());
    pipe.take(MAX_HANDOVER_BYTES)
        .read_to_end(&mut encoded)
        .context("reading the Windows handover")?;
    let mut handover = parse_handover(&encoded, HANDOVER_VERSION)?;
    handover.lifetime_lock = lifetime_lock;
    Ok(handover)
}

/// The lock handle announced after the pipe's, if the previous daemon sent
/// one. One from before the lock closes stdin after the pipe's line.
fn receive_lifetime_lock(announce: &mut impl BufRead) -> Option<File> {
    let mut line = String::new();
    announce.read_line(&mut line).ok()?;
    let value = line
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|value| *value != 0)?;
    if !is_file_of_type(value, FILE_TYPE_DISK) {
        log::warn!("the previous daemon's lifetime lock handle is not a file; not adopted");
        return None;
    }
    // SAFETY: as for the pipe: the previous daemon duplicated this handle into
    // this process for it to own, nothing else here knows its value, and it
    // is a file.
    Some(unsafe { File::from_raw_handle(value as _) })
}

/// Whether the handle value `value` names an open object of `kind`. Asking
/// only reads: an invalid value is reported as `FILE_TYPE_UNKNOWN`.
fn is_file_of_type(value: usize, kind: FILE_TYPE) -> bool {
    // SAFETY: `GetFileType` validates the handle and reads nothing through it
    // that a closed or foreign value could make unsafe.
    (unsafe { GetFileType(HANDLE(value as _)) }) == kind
}

fn parse_handover(encoded: &[u8], version: u32) -> Result<Handover> {
    let handover: Handover =
        serde_json::from_slice(encoded).context("parsing the Windows handover")?;
    anyhow::ensure!(
        handover.version == version,
        "handover version {} is not the {version} this transport carries",
        handover.version
    );
    handover.retention.validate()?;
    Ok(handover)
}

pub fn mark_ready(path: &Path) -> Result<()> {
    crate::catalog::write_private_file(path, b"ready").with_context(|| {
        format!(
            "publishing Windows replacement readiness {}",
            path.display()
        )
    })
}

/// Checks that the candidate understands this handover before the old daemon
/// stops. This is the irreversible boundary on Windows: after the old process
/// exits there is no image left that can safely own the session metadata.
pub fn replacement_accepts_handover(executable: &Path) -> Result<bool> {
    let status = Command::new(executable)
        .arg("--resume-check")
        .arg(HANDOVER_VERSION.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("running {}", executable.display()))?;
    Ok(status.success())
}

/// Starts the replacement, which then waits on its stdin for
/// [`send_handover`].
pub fn spawn_replacement(executable: &Path, ready: &Path) -> Result<Child> {
    use std::os::windows::process::CommandExt as _;

    Command::new(executable)
        .args([
            "--daemon".to_owned(),
            "--resume-from-pipe".to_owned(),
            format!("--resume-ready={}", ready.display()),
        ])
        .creation_flags(0x0800_0000)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        // Keep replacement diagnostics on the daemon's stderr. Test harnesses
        // redirect that stream to their per-daemon log, and normal detached
        // launches retain their existing stderr policy through inheritance.
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("starting replacement multiplexer {}", executable.display()))
}

/// Waits until the candidate has read and validated the handover. It is still
/// waiting for the old daemon to release the endpoint, so the caller can now
/// stop the old listener without racing an unvalidated replacement.
pub fn wait_for_ready(child: &mut Child, ready: &Path) -> Result<()> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if fs::read(ready).is_ok_and(|contents| contents == b"ready") {
            return Ok(());
        }
        if let Some(status) = child
            .try_wait()
            .context("checking the Windows replacement")?
        {
            anyhow::bail!("the Windows replacement exited before becoming ready ({status})");
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "the Windows replacement did not become ready within {READY_TIMEOUT:?}"
        );
        std::thread::sleep(READY_POLL);
    }
}

#[cfg(test)]
#[path = "tests/upgrade_windows.rs"]
mod tests;
