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
    os::windows::io::{AsRawHandle as _, FromRawHandle as _},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use windows::Win32::{
    Foundation::{DUPLICATE_CLOSE_SOURCE, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE},
    Storage::FileSystem::{FILE_TYPE_PIPE, GetFileType},
    System::{Pipes::CreatePipe, Threading::GetCurrentProcess},
};
use zeroize::Zeroizing;

use crate::messages::ClientId;

/// Bumped whenever the private Windows handover changes incompatibly. Optional
/// defaulted fields keep the version so an older daemon can upgrade into them.
///
/// 4 moved the handover from a file to a pipe, which `--resume-check` has to
/// tell an older image about: one that cannot read the pipe refuses before the
/// old daemon starts it, rather than after.
pub const HANDOVER_VERSION: u32 = 4;

/// The last version a daemon wrote to a file. A daemon from before the pipe
/// still upgrades *into* this image that way, so it is still read — the old
/// image has already written it — and removed as soon as it has been.
pub const FILE_HANDOVER_VERSION: u32 = 3;

/// The largest handover a replacement reads. Retained output is bounded per
/// pane, so this only stops a broken sender from exhausting memory.
const MAX_HANDOVER_BYTES: u64 = 1 << 30;

/// Whether this image can adopt a handover of `version`, which is what
/// `--resume-check` answers.
pub fn accepts_handover_version(version: u32) -> bool {
    version == HANDOVER_VERSION || version == FILE_HANDOVER_VERSION
}

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

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handover {
    pub version: u32,
    pub generation: u64,
    pub next_session_id: u64,
    pub next_pane_id: u64,
    pub retention: crate::retention::Retention,
    pub sessions: Vec<SessionHandover>,
    /// Which recipients each client configured, so a session protected after
    /// the upgrade is still sealed to its own client's choice. Absent from an
    /// older image; see `server::sealing::adopted_grants`.
    #[serde(default)]
    pub recipient_grants: Option<crate::server::RecipientGrants>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionHandover {
    pub id: u64,
    pub summary: crate::protocol::BackgroundSessionSummary,
    pub state: serde_json::Value,
    /// Canonical collaboration state, if this session was shared. Optional so
    /// a handover from before collaboration can still be upgraded safely.
    ///
    /// As [`crate::upgrade::SessionHandover::shared_state`] on Unix. The two
    /// structures describe the same handover and are read by the same code in
    /// `server/upgrade.rs`, so a field added to one has to be added to the
    /// other — this one was missed, and Windows has not compiled since.
    #[serde(default)]
    pub shared_state: Option<crate::messages::SharedSessionState>,
    pub keep: bool,
    pub offered: bool,
    #[serde(default)]
    pub owner: Option<u32>,
    pub verifier: Option<String>,
    /// As [`crate::upgrade::SessionHandover::key_envelope`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_envelope: Option<String>,
    pub failed_authentications: u32,
    pub refuse_for: Option<Duration>,
    /// The recipients a protected session's records are pinned to. Carried
    /// because the replacement cannot tell them from whatever the store was
    /// last configured with; absent from an older image, which pinned nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed_to: Option<Vec<String>>,
    pub panes: Vec<PaneHandover>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneHandover {
    pub id: u64,
    /// The stable identifier of the console held by the zmux-pty host.
    pub console_id: u64,
    pub child_pid: u32,
    pub attachment: AttachmentHandover,
    #[serde(default)]
    pub attachment_client_id: Option<ClientId>,
    pub columns: u16,
    pub lines: u16,
    pub exited: bool,
    pub exit_status: Option<i32>,
    pub retained: Vec<u8>,
    /// The pane's `SSH_AUTH_SOCK` pipe, absent where the old daemon was not
    /// serving one — or, in a version 3 handover, where it is the predictable
    /// name every pane had then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_pipe: Option<String>,
    /// An instance of [`Self::agent_pipe`] the old daemon duplicated into the
    /// replacement, as a handle value in the replacement's own table. Holding
    /// it keeps the name from ever being free for another account to create.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_pipe_instance: Option<u64>,
}

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
    #[serde(default)]
    pub client_id: crate::messages::ClientId,
    #[serde(default)]
    pub stream_only: bool,
    #[serde(default)]
    pub columns: Option<u16>,
    #[serde(default)]
    pub lines: Option<u16>,
    pub input_sent: bool,
}

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
pub fn send_handover(replacement: &mut Child, handover: &Handover) -> Result<()> {
    let stdin = replacement
        .stdin
        .take()
        .context("the Windows replacement has no stdin")?;
    send_handover_to(HANDLE(replacement.as_raw_handle()), stdin, handover)
}

fn send_handover_to(process: HANDLE, mut announce: impl Write, handover: &Handover) -> Result<()> {
    let encoded =
        Zeroizing::new(serde_json::to_vec(handover).context("serializing the Windows handover")?);
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    // Not inheritable: the replacement is given the read end explicitly, and
    // nothing else this daemon starts may receive either end.
    unsafe { CreatePipe(&mut read, &mut write, None, 0) }
        .context("creating the Windows handover pipe")?;
    // SAFETY: CreatePipe returned this handle, which nothing else owns.
    let mut writer = unsafe { File::from_raw_handle(write.0 as _) };
    let mut remote = HANDLE::default();
    // DUPLICATE_CLOSE_SOURCE closes this process's read end even on failure,
    // which leaves the replacement's copy the only one.
    unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            read,
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
            parse_handover(&encoded?, FILE_HANDOVER_VERSION)
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
    // SAFETY: the previous daemon duplicated this handle into this process
    // for it to own, and nothing else here knows its value.
    let pipe = unsafe { File::from_raw_handle(value as _) };
    anyhow::ensure!(
        unsafe { GetFileType(HANDLE(pipe.as_raw_handle())) } == FILE_TYPE_PIPE,
        "the handover handle is not a pipe"
    );
    let mut encoded = Zeroizing::new(Vec::new());
    pipe.take(MAX_HANDOVER_BYTES)
        .read_to_end(&mut encoded)
        .context("reading the Windows handover")?;
    parse_handover(&encoded, HANDOVER_VERSION)
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
