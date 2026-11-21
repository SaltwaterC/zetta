//! What an upgrade hands the replacement daemon, which is the same on every
//! platform but for the pieces each one carries as an operating-system object:
//! a pane's terminal (a descriptor that survives `execv` on Unix, a console the
//! zmux-pty host keeps on Windows) and the lifetime lock. Those are the fields
//! under a `cfg` below; everything else is declared once, because a field added
//! to only one platform's copy once left the other not compiling. Included by
//! both `upgrade.rs` and `upgrade_windows.rs`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::messages::ClientId;

/// Bumped whenever [`Handover`] changes incompatibly. Optional fields with a
/// serde default deliberately keep the version so an older running daemon can
/// hand its state to the build that introduced them.
pub const HANDOVER_VERSION: u32 = 1;

/// Whether this image can adopt a handover of `version`, which is what
/// `--resume-check` answers.
pub fn accepts_handover_version(version: u32) -> bool {
    version == HANDOVER_VERSION
}

/// Everything the next image needs to carry on.
///
/// On Unix, the listening socket is passed separately as an inherited
/// descriptor on platforms that preserve its local peer-credential state across
/// `execv`. It is not serialized in this blob because it is already an
/// operating-system object. macOS rebinds it in the replacement instead,
/// because inheriting a Darwin local socket loses the peer credentials used to
/// authorize requests.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handover {
    pub version: u32,
    pub generation: u64,
    pub next_session_id: u64,
    pub next_pane_id: u64,
    /// The policy the replaced daemon was actually using.
    ///
    /// Older Unix handovers omitted it and therefore resume as the historical
    /// in-memory default. New handovers must carry it: resuming a disk daemon
    /// as memory recovery marks its still-live records restorable. Every
    /// Windows handover has carried it, so there it stays required.
    #[cfg_attr(unix, serde(default))]
    pub retention: crate::retention::Retention,
    pub sessions: Vec<SessionHandover>,
    /// Which recipients each client configured, so a session protected after
    /// the upgrade is still sealed to its own client's choice. Absent from an
    /// older image; see `server::sealing::adopted_grants`.
    #[serde(default)]
    pub recipient_grants: Option<crate::server::RecipientGrants>,
    /// The descriptor holding the daemon's lifetime lock, kept open across the
    /// `execv` so the lock is never free while the image is replaced (see
    /// `server/lifetime_lock.rs`). Optional when no lock was inherited.
    #[cfg(unix)]
    #[serde(default)]
    pub lifetime_lock: Option<std::os::fd::RawFd>,
    /// The daemon's lifetime lock, as a handle the old daemon duplicated into
    /// this process (see `server/lifetime_lock.rs`). Never serialized: it is
    /// announced on stdin after the pipe's handle, so a daemon from before the
    /// lock, which announces only the pipe, still upgrades into this image, and
    /// this image still upgrades into one that ignores the extra line.
    #[cfg(windows)]
    #[serde(skip)]
    pub lifetime_lock: Option<std::fs::File>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionHandover {
    pub id: u64,
    pub summary: crate::protocol::BackgroundSessionSummary,
    pub state: serde_json::Value,
    /// Canonical collaboration state, if this session was shared. Optional so
    /// a handover from before collaboration can still be upgraded safely.
    #[serde(default)]
    pub shared_state: Option<crate::messages::SharedSessionState>,
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
    /// When `owner` started, so the replacement goes on recognising the owner
    /// rather than whichever process holds its PID by then; see
    /// `process_status::ProcessIdentity`. Absent from an older image, and where
    /// the platform cannot tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_started: Option<u64>,
    /// The Argon2id verifier, so a protected session stays protected across an
    /// upgrade.
    pub verifier: Option<String>,
    /// The sealed session key that goes with `verifier`, so an automatically
    /// protected session stays *openable* across an upgrade. Losing it would
    /// leave the session protected by a key nobody can recover, which is
    /// indistinguishable from having killed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_envelope: Option<String>,
    /// Consecutive wrong secrets, and what is left of the window they opened.
    ///
    /// Carried rather than reset: dropping them would make `--upgrade` a way to
    /// clear a session's rate limit, which is exactly the thing the backoff
    /// exists to prevent. Stored as a remaining duration because the deadline
    /// is a monotonic instant with no meaning in another image.
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
    /// The PTY master, which survives the `execv` and is reclaimed by number.
    ///
    /// No child-event socket travels with it. The daemon is the child's parent
    /// on both sides of an `execv` — that is the entire reason the upgrade is a
    /// re-exec — so the replacement reaps with `waitpid` and registers a fresh
    /// `SIGCHLD` pipe. Carrying the old pipe instead handed the new image a
    /// descriptor whose writer the exec had already closed, which read as end of
    /// file and so as "the process ended", falsely, for every pane that had been
    /// attached.
    #[cfg(unix)]
    pub descriptor: std::os::fd::RawFd,
    /// The stable identifier of the console held by the zmux-pty host.
    #[cfg(windows)]
    pub console_id: u64,
    pub child_pid: u32,
    /// Who held this pane, so the replacement offers it in the same mode. A
    /// client that dies across the upgrade is then still noticed by the
    /// liveness reclaim, and a pane mid-handover can still complete it.
    pub attachment: AttachmentHandover,
    /// The exact logical request that acquired an exclusive attachment.
    /// Older daemon images did not carry this, so an in-place upgrade accepts
    /// its absence and falls back to PID ownership until the next attach.
    #[serde(default)]
    pub attachment_client_id: Option<ClientId>,
    /// The size the pane is running at, so arbitration continues from what
    /// every viewer is showing rather than restarting from a default.
    pub columns: u16,
    pub lines: u16,
    pub exited: bool,
    /// The status observed if it has already ended, so a client asking what it
    /// missed gets the real exit rather than "status unavailable".
    pub exit_status: Option<i32>,
    pub retained: Vec<u8>,
    /// The pane's `SSH_AUTH_SOCK` pipe, absent where the previous daemon was
    /// not serving one.
    #[cfg(windows)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_pipe: Option<String>,
    /// An instance of [`Self::agent_pipe`] the old daemon duplicated into the
    /// replacement, as a handle value in the replacement's own table. Holding
    /// it keeps the name from ever being free for another account to create.
    #[cfg(windows)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_pipe_instance: Option<u64>,
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
    /// The logical client survives request-connection and SSH-forward
    /// reconnects, unlike the process ID which is shared by every pane in a
    /// window and may be reported identically by forwarded peers.
    #[serde(default)]
    pub client_id: crate::messages::ClientId,
    #[serde(default)]
    pub stream_only: bool,
    /// A client may still be waiting for its first initialized layout when the
    /// daemon is replaced, so the measurement is carried as optional state.
    #[serde(default)]
    pub columns: Option<u16>,
    #[serde(default)]
    pub lines: Option<u16>,
    /// Kept because a pane's exit reports which viewers typed into it, and an
    /// upgrade must not launder that away.
    pub input_sent: bool,
}
