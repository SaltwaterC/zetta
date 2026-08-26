//! Replacing the Windows daemon while keeping its sessions.
//!
//! Windows cannot replace a running executable in place, and a pseudoconsole
//! belongs to the process that created it. The consoles therefore live in the
//! long-lived zmux-pty host; this module carries the daemon's ordinary session
//! state through a short-lived private handover file while the old daemon
//! stops and its successor binds the same endpoint.

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

/// Bumped whenever the private Windows handover changes shape.
pub const HANDOVER_VERSION: u32 = 1;

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
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionHandover {
    pub id: u64,
    pub summary: crate::protocol::BackgroundSessionSummary,
    pub state: serde_json::Value,
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
    pub columns: u16,
    pub lines: u16,
    pub exited: bool,
    pub exit_status: Option<i32>,
    pub retained: Vec<u8>,
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
    pub columns: u16,
    pub lines: u16,
    pub input_sent: bool,
}

/// Writes the state in the daemon's private session directory. The random,
/// create-new name means a stale file can never be silently overwritten, and
/// the directory is already restricted by create_private_dir.
pub fn write_handover(directory: &Path, handover: &Handover) -> Result<(PathBuf, PathBuf)> {
    let encoded = serde_json::to_vec(handover).context("serializing the Windows handover")?;
    for _ in 0..3 {
        let stem = format!("zmux-handover-{}", crate::transport::random_hex(16)?);
        let path = directory.join(format!("{stem}.json"));
        let ready = directory.join(format!("{stem}.ready"));
        let mut file = match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("creating Windows handover {}", path.display()));
            }
        };
        file.write_all(&encoded)
            .with_context(|| format!("writing Windows handover {}", path.display()))?;
        return Ok((path, ready));
    }
    anyhow::bail!("could not allocate a unique Windows handover path")
}

pub fn read_handover(path: &Path) -> Result<Handover> {
    let encoded =
        fs::read(path).with_context(|| format!("reading Windows handover {}", path.display()))?;
    let handover: Handover = serde_json::from_slice(&encoded)
        .with_context(|| format!("parsing Windows handover {}", path.display()))?;
    anyhow::ensure!(
        handover.version == HANDOVER_VERSION,
        "handover version {} is not the {HANDOVER_VERSION} this multiplexer understands",
        handover.version
    );
    handover.retention.validate()?;
    Ok(handover)
}

pub fn remove_handover(path: &Path, ready: &Path) {
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(ready);
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

pub fn spawn_replacement(executable: &Path, handover: &Path, ready: &Path) -> Result<Child> {
    use std::os::windows::process::CommandExt as _;

    Command::new(executable)
        .args([
            "--daemon".to_owned(),
            format!("--resume-from={}", handover.display()),
            format!("--resume-ready={}", ready.display()),
        ])
        .creation_flags(0x0800_0000)
        .stdin(Stdio::null())
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
