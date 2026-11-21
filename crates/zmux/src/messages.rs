//! The client/daemon wire protocol.
//!
//! Deliberately transport-agnostic: nothing here depends on descriptor passing
//! or on peer credentials, so the same messages can later carry a session over
//! a remote transport. Descriptor passing is an optimisation the local
//! transport applies to [`Response::Spawned`] and [`Response::Attached`].

use std::{collections::HashMap, path::PathBuf};

use alacritty_terminal::tty::ConsolePalette;
use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::auth::SessionSecret;
use crate::protocol::{BackgroundPaneLayout, BackgroundSessionSummary, RestorableSessionRecord};

mod shared;
pub use shared::*;
mod layout;
pub use layout::*;

/// The wire format, and what a client and a multiplexer compare before they
/// trust each other to understand one another.
pub const PROTOCOL_VERSION: u32 = 1;

/// Version of the durable collaboration envelope. This is independent from
/// [`PROTOCOL_VERSION`]: a daemon upgrade may keep a session state produced by
/// an older binary even when the live wire protocol has moved on.
pub const SHARED_SESSION_STATE_VERSION: u32 = 1;

/// The opening of the refusal a multiplexer sends a client whose protocol
/// version it does not speak.
///
/// Fixed wording, and deliberately not reworded: the two sides of this message
/// are different builds by definition, and the side that has to *recognize* one
/// is always the older. A refusal is an ordinary [`Response::Error`] — giving
/// it a variant of its own would only be understood by a client new enough to
/// parse the variant, which is never the client being refused.
const PROTOCOL_MISMATCH_PREFIX: &str = "this multiplexer speaks protocol version";

/// How a multiplexer says it does not speak a client's protocol version.
pub fn protocol_mismatch_message(speaks: u32, spoken: u32) -> String {
    format!("{PROTOCOL_MISMATCH_PREFIX} {speaks}, not {spoken}")
}

/// Whether a refusal is that message, including one from a daemon built before
/// [`protocol_mismatch_message`] existed.
///
/// `contains` rather than an exact match, because by the time a client tests
/// this the message has usually been wrapped in the context of whatever request
/// it refused.
pub fn is_protocol_mismatch_message(message: &str) -> bool {
    message.contains(PROTOCOL_MISMATCH_PREFIX)
}

/// Maximum encoded image size accepted by the image-paste request.
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

/// Every request carries the endpoint token, which authenticates the *channel*
/// only. It says nothing about whether a protected session may be attached —
/// that needs the session's own secret, checked against its verifier.
/// Deliberately tolerant of unknown fields, unlike everything it carries.
///
/// The version is inside the message, so refusing to parse a message that has
/// an unfamiliar field means never getting far enough to report the mismatch.
/// A client built against a newer protocol must get a version error, not a
/// closed connection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u32,
    pub token: String,
    /// The client's own process.
    ///
    /// Two jobs, both on every platform. It is where a terminal is duplicated
    /// *to* where the platform cannot attach one to a message — on Unix the
    /// descriptor travels with the reply instead. And it is the client's
    /// *identity*: which client holds a pane, so a pane can be reclaimed when
    /// that client dies, a revoke can be addressed to the one holder, and
    /// releasing or detaching a pane can be refused to a client that is not
    /// holding it.
    #[serde(default)]
    pub client_process_id: u32,
    /// A stable identity for this logical client. Unlike the process ID it is
    /// unique across clients using one process, and survives reconnects after
    /// a daemon replacement or SSH forward restart.
    #[serde(default)]
    pub client_id: ClientId,
    /// Remote clients cannot receive descriptors or participate in exclusive
    /// handover. They attach through the shared byte-stream path only.
    #[serde(default)]
    pub stream_only: bool,
    /// Authentication for stream-safe administrative requests. Attach keeps
    /// its credential in [`Request::Attach`] because it is the first request
    /// on the long-lived shared data connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_secret: Option<SessionSecret>,
    pub request: Request,
}

/// The logical identity of one mux client.
///
/// Process IDs are intentionally not enough: two panes opened through one
/// remote Zetta process use the same process ID, and the SSH server may report
/// the same peer process for every forwarded connection. A random ID gives
/// shared-client routing and upgrade handover an unambiguous key without
/// changing local PID ownership and liveness semantics.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientId(String);

impl ClientId {
    pub fn random() -> anyhow::Result<Self> {
        Ok(Self(crate::transport::random_hex(16)?))
    }

    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SharedSessionState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        let wire = SharedSessionStateWire::deserialize(deserializer)?;
        SharedSessionState {
            version: wire.version,
            session_id: wire.session_id,
            revision: wire.revision,
            last_operation_id: wire.last_operation_id,
            operation_receipts: wire.operation_receipts,
            summary: wire.summary,
            presentation: wire.presentation,
            state: wire.state,
        }
        .migrate()
        .map_err(D::Error::custom)
    }
}

/// The fields of [`Request::Attach`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttachRequest {
    pub session_id: u64,
    pub pane_id: Option<u64>,
    pub secret: Option<SessionSecret>,
    /// Refuses an exclusive descriptor handoff. Batch-created shared panes
    /// use this even for a local client so every viewer follows the same
    /// relay path.
    #[serde(default)]
    pub force_shared: bool,
    /// The client this attachment is a *relay* for, when it is one.
    ///
    /// `zmux relay-pane` attaches a pane on the daemon's own host so its
    /// bytes can travel to a window over Mosh instead of over the SSH
    /// forward. That window is therefore not in the pane's shared set, and
    /// a control request of its own — image paste — has no way to show it
    /// is looking at the pane. This is that way: the relay names the viewer
    /// it is showing the pane to, and the daemon treats that viewer as
    /// present for exactly as long as the relay's own attachment lasts.
    ///
    /// It grants nothing the declaring client does not already have: a
    /// client that can attach this pane is a viewer of it and can paste
    /// into it itself.
    ///
    /// A [`PROTOCOL_VERSION`] change, unlike [`Request::ReleaseExclusive`],
    /// even though a daemon that predates the field would merely ignore it.
    /// What cannot be ignored is the relay that sets it: Zetta starts it as
    /// `zmux relay-pane --viewer-stdin`, and a remote executable that
    /// predates the option refuses to run at all, so the pane would close
    /// the instant it opened. The version is what turns that into a
    /// mismatch the user is told how to fix, before any pane is brought up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relaying_for: Option<ClientId>,
}

/// The fields of [`Request::Snapshot`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotRequest {
    pub session_id: u64,
    pub pane_id: u64,
    /// Length of the raw bytes that follow the message on the connection.
    pub length: usize,
    pub columns: u16,
    pub lines: u16,
}

/// The fields of [`Request::StoreImage`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoreImageRequest {
    pub session_id: u64,
    pub pane_id: u64,
    /// Length of the raw PNG bytes that follow the message.
    pub length: usize,
}

/// The fields of [`Request::SetSessionScope`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionScopeRequest {
    pub session_id: u64,
    pub shared: bool,
    /// The Argon2id verifier for the secret a joining process must present.
    ///
    /// Required when sharing a session that has none: a session another
    /// process can join unchallenged hands it whatever its terminals can
    /// already do, which for a shell that has answered `sudo` is root.
    pub verifier: Option<String>,
    /// The sealed session key that goes with `verifier`, when the secret was
    /// generated rather than typed. See
    /// [`crate::protocol::BackgroundSessionSummary::key_envelope`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_envelope: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum Request {
    /// Confirms that the daemon has completed startup and can answer requests.
    Ping,
    /// Starts a process under the multiplexer and hands its terminal back.
    Spawn(SpawnRequest),
    /// Creates a pane owned by the daemon and attaches the caller through the
    /// shared byte-stream path. No descriptor is ever returned.
    SpawnShared(SharedSpawnRequest),
    /// Atomically starts every draft process and commits one canonical layout.
    /// The response is stream-free; callers attach every returned pane through
    /// the same shared path used by remote viewers.
    SpawnSharedBatch(SharedSpawnBatchRequest),
    /// Creates a complete headless shared session. The response is stream-free;
    /// callers attach its panes later through the ordinary shared attach path.
    CreateShared(CreateSharedRequest),
    /// Takes over a pane's terminal from the multiplexer.
    ///
    /// `pane_id` is absent for the session's first pane, which is how an
    /// attach starts: the caller cannot know a protected session's pane
    /// identifiers, because they are not published until it has authenticated.
    Attach(AttachRequest),
    /// Gives the multiplexer a screen checkpoint from the client that is
    /// showing a pane. During a revoke handover this is the screen that lets
    /// the multiplexer resume reading and relay the pane to every client that
    /// attaches. A live share uses the same one-shot message while the client
    /// remains exclusive, so a persisted offer does not start with an empty
    /// screen.
    ///
    /// Sent over a fresh connection from the client showing the pane. During a
    /// revoke it follows [`Event::Revoke`]; during a live share it precedes the
    /// [`Request::Share`] publication. The raw snapshot bytes follow the
    /// message. The connection carries nothing else and closes afterwards.
    ///
    /// `columns`/`lines` are the size the holder was showing the pane at. The
    /// daemon uses them as the pane's current applied size; shared clients
    /// still remain unmeasured until they report their own initialized layout.
    Snapshot(SnapshotRequest),
    /// Stores a normalized clipboard image for a shared pane and returns the
    /// absolute path the attached TUI can consume. The raw PNG bytes follow
    /// the request on the connection.
    StoreImage(StoreImageRequest),
    /// Input from a shared client, on the shared connection [`Request::Attach`]
    /// left open after it was answered with [`Response::SharedAttached`].
    ///
    /// The raw bytes follow the message.
    Input {
        /// Length of the raw bytes that follow the message on the connection.
        length: usize,
    },
    /// Asks to be identified before the real request is sent, answered with
    /// [`Response::Ok`] once the exchange is over — successfully or not.
    ///
    /// How a request that streams raw bytes after its message gets an identity:
    /// the daemon cannot interject a challenge into one of those, because the
    /// bytes would arrive where it was expecting the answer.
    Attest,
    /// Proves, where the platform has no peer credentials, that this connection
    /// really belongs to the process its envelope named: the nonce read back
    /// from the handle in [`Response::AttestationRequired`].
    ///
    /// Sent only in answer to that response, on the same connection, after
    /// which the request that prompted it is dispatched.
    Attested { nonce: String },
    /// Gives a session back to the multiplexer to hold. The client has already
    /// stopped reading the panes' terminals by the time this is sent.
    Detach(DetachRequest),
    /// Restores an encrypted disk record after the client has decrypted it.
    /// The daemon receives state and authentication metadata, never an age
    /// identity or a private key. The raw snapshots described by the request
    /// follow the JSON frame on the same connection.
    Resume(Box<ResumeRequest>),
    /// Takes a shared pane's terminal back, in answer to [`Event::Grant`].
    ///
    /// The reverse of the revoke handover. Only the pane's single remaining
    /// viewer may send it, and the multiplexer answers with the descriptor
    /// exactly as it answers an exclusive [`Request::Attach`] — except that no
    /// replay comes with it, because everything read so far has already been
    /// relayed to this very client. What is left is still in the terminal, for
    /// the client to read itself from now on.
    ///
    /// Sent on a fresh connection, like [`Request::Snapshot`]: the shared
    /// connection is being retired, and the multiplexer closes its end of it once
    /// the last relayed frame has gone out.
    TakeExclusive { session_id: u64, pane_id: u64 },
    /// Gives a pane back after the client that took it could not use it.
    ///
    /// [`Request::TakeExclusive`] hands the descriptor over before the client
    /// knows whether its terminal can adopt it, and a client that fails at that
    /// point — or whose window closes as the answer arrives — is left holding a
    /// pane it does not read. The daemon does not read one either, because the
    /// holder is supposed to, so the pty fills, the shell blocks on its own
    /// output and stops reading input: the pane goes dead for every viewer of
    /// it, not only for the client that asked.
    ///
    /// A holder that has died is recovered by the liveness sweep. This is for
    /// the one that is still running, which nothing else can detect.
    ///
    /// Deliberately not a [`PROTOCOL_VERSION`] change: a daemon that predates
    /// this request answers it with an error, which is what a daemon that
    /// cannot release a pane should say, and bumping the version would instead
    /// refuse every remote session whose two ends were built a day apart.
    ReleaseExclusive { session_id: u64, pane_id: u64 },
    /// Offers a session that the client is still showing, so another client can
    /// attach to it and both then see the same panes.
    ///
    /// Deliberately not [`Request::Detach`] with the snapshots left out. A
    /// detach means "I have stopped reading these terminals, hold them for me",
    /// and it is what asks for the session to outlive its window; sharing means
    /// neither. Conflating the two made joining a live session require first
    /// dismissing it, which is the opposite of what the user asked for.
    Share(ShareRequest),
    /// Tells the multiplexer that an attached pane was resized.
    ///
    /// Needed where the pseudoconsole belongs to the multiplexer and only it
    /// can resize the console. On Unix the resize has already happened through
    /// the descriptor the client holds.
    Resize {
        session_id: u64,
        pane_id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revision: Option<SessionRevision>,
        columns: u16,
        lines: u16,
    },
    /// Updates the legacy Win32 colors for one pane's pseudoconsole.
    SetConsolePalette {
        session_id: u64,
        pane_id: u64,
        palette: ConsolePalette,
    },
    /// The sessions being held, as published in the catalog.
    List,
    /// Ends a session and everything running in it.
    Kill { session_id: u64 },
    /// Scopes a session to one process, or shares it with every process.
    ///
    /// The CLI half of what `Ctrl-Shift-K` does to a tab on screen, for a
    /// session that is in the background and therefore has no window to toggle
    /// it from. Sharing needs no owner; scoping back needs one, and it is the
    /// process recorded when the session was last held — not the caller, which
    /// for a CLI is a process that exits a moment later.
    SetSessionScope(SessionScopeRequest),
    /// Removes a session from the catalog without killing it. The session
    /// continues running under the daemon but is no longer listed or
    /// attachable until the daemon restarts (at which point it is gone).
    Forget { session_id: u64 },
    /// Applies the current client's session retention settings to the daemon.
    ///
    /// A daemon outlives the client that started it, so a later client must not
    /// silently inherit whatever retention mode the earlier client selected.
    /// The recipient strings have already been resolved by the client; they
    /// are public age recipients, never identities or private keys.
    Configure {
        retention: crate::retention::Retention,
        persistence_recipients: Vec<String>,
    },
    /// Turns this connection into an event stream. No response follows; the
    /// daemon sends [`Event`]s until the connection is dropped.
    Subscribe,
    /// What the multiplexer currently knows about these panes.
    ///
    /// A client that lost its subscription — across an upgrade, or a daemon
    /// restart — missed every [`Event::PaneExited`] sent while it was away, and
    /// those events are broadcast to whoever is listening rather than queued.
    /// Asking directly on reconnect is what closes that hole: without it a pane
    /// whose process ended during the gap would wait for a notification that
    /// has already been and gone.
    PaneStates { pane_ids: Vec<u64> },
    /// Applies one collaboration mutation against an exact canonical revision.
    ApplyShared(SharedSessionOperationRequest),
    /// Creates or retrieves the complete canonical collaboration state.
    SharedSnapshot { session_id: u64 },
    /// Explicitly leaves a shared session without changing its daemon-owned
    /// panes. Closing a remote tab uses this rather than `ClosePane`.
    LeaveShared { session_id: u64 },
    /// Releases a pane whose window closed while its process was still running.
    ///
    /// Dropping the client's descriptor is not enough to tell the multiplexer
    /// anything: it holds its own, so the pane stays marked as taken, nobody
    /// drains it, and the program blocks as soon as the terminal's buffer
    /// fills. This hands the pane back so it is either drained or, if the
    /// session was never meant to outlive its window, ended.
    ClosePane { session_id: u64, pane_id: u64 },
    /// Permanently removes an exclusively held pane after an explicit close.
    ///
    /// Unlike `ClosePane`, this removes the daemon-owned PTY even when sibling
    /// panes or a kept session remain. Older daemons reject this request, so no
    /// protocol version change is needed.
    DiscardPane { session_id: u64, pane_id: u64 },
    /// Stops the daemon once it is holding nothing.
    Shutdown,
    /// Replaces the daemon with a fresh image of itself, keeping every session.
    /// The image is the one the daemon resolved at startup; a client cannot
    /// choose it, because choosing it would mean inheriting the terminals of
    /// every protected session the daemon holds.
    Upgrade,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnRequest {
    /// Adds a pane to an existing session, or starts a new one when absent.
    pub session_id: Option<u64>,
    /// As [`Envelope::client_process_id`].
    #[serde(default)]
    pub client_process_id: u32,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub working_directory: Option<PathBuf>,
    pub size: TerminalSize,
    pub console_palette: ConsolePalette,
    /// As [`SharedSpawnRequest::console_backend`].
    #[serde(
        default,
        skip_serializing_if = "zetta_profiles::ConsoleBackend::is_default"
    )]
    pub console_backend: zetta_profiles::ConsoleBackend,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalSize {
    pub columns: u16,
    pub lines: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetachRequest {
    pub session_id: u64,
    /// What the catalog publishes for this session.
    pub summary: BackgroundSessionSummary,
    /// The client's own session state, round-tripped without being read. Tab
    /// layout, labels and flags belong to the application, so keeping them
    /// opaque means a new application feature needs no daemon change.
    pub state: serde_json::Value,
    /// An Argon2id verifier, when reattaching is to require a secret. Absent
    /// leaves an already-protected session's verifier as it is.
    pub verifier: Option<String>,
    /// The sealed session key that goes with `verifier`, when the secret was
    /// generated rather than typed. See
    /// [`crate::protocol::BackgroundSessionSummary::key_envelope`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_envelope: Option<String>,
    /// Per pane, the screen the client was showing, replayed on the next
    /// attach so reattaching does not start from a blank terminal.
    pub snapshots: Vec<PaneSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeRequest {
    pub record_id: u64,
    pub summary: BackgroundSessionSummary,
    pub state: serde_json::Value,
    /// The daemon-owned collaboration envelope, when the restored record was
    /// shared. Keeping it in the resume request preserves the canonical
    /// revision instead of silently starting a new collaboration history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_state: Option<SharedSessionState>,
    pub verifier: Option<String>,
    /// As [`DetachRequest::key_envelope`], read back out of the record the
    /// client has just decrypted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_envelope: Option<String>,
    pub failed_authentications: u32,
    pub backoff_seconds: u64,
    pub created_at: u64,
    pub updated_at: u64,
    /// The session secret is sent only after the client has decrypted the
    /// record. It is checked by the daemon and then discarded before the
    /// restored record is kept in memory.
    pub secret: Option<SessionSecret>,
    /// Per-pane lengths for the raw snapshots that follow this message on the
    /// connection. Keeping the screen bytes out of the JSON frame matters for
    /// a disk record with a large scrollback buffer: control messages are
    /// deliberately capped, while snapshots already have their own retention
    /// limit and framing.
    pub snapshots: Vec<ResumeSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeSnapshot {
    pub pane_id: u64,
    /// Number of raw bytes following the request for this pane.
    pub length: usize,
}

/// Publishes a session the client is still showing, so other clients can find
/// and attach to it.
///
/// Carries the same summary and state a detach does, and for the same reason:
/// the catalog needs something to list, and a client that joins rebuilds its tab
/// from the state. Snapshot bytes are sent separately with [`Request::Snapshot`]
/// immediately beforehand: the panes are still being read by the sharing
/// client, so the share publication itself must not carry a large raw payload.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareRequest {
    pub session_id: u64,
    /// What the catalog publishes for this session.
    pub summary: BackgroundSessionSummary,
    /// As [`DetachRequest::state`].
    pub state: serde_json::Value,
    /// As [`DetachRequest::verifier`].
    pub verifier: Option<String>,
    /// As [`DetachRequest::key_envelope`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_envelope: Option<String>,
    /// Whether the session is being offered or withdrawn.
    ///
    /// Withdrawing stops the session being listed and attachable; it does not
    /// evict clients that already joined, because there is no way to give a pane
    /// back to one viewer exclusively while another is still relaying it.
    pub offered: bool,
}

/// What the multiplexer knows about one pane, for a client catching up after
/// its subscription was interrupted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneStateReport {
    pub pane_id: u64,
    /// The multiplexer is not holding this pane at all: it ended and was
    /// pruned, or its session was killed. Either way there is nothing left to
    /// wait for, which a client must be able to distinguish from "still
    /// running" — the two look identical from a terminal that cannot reap.
    pub unknown: bool,
    pub exited: bool,
    /// The raw status the multiplexer observed, when it observed one.
    pub raw_status: Option<i32>,
    /// As [`Event::PaneExited::input_sent`].
    #[serde(default)]
    pub input_sent: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneSnapshot {
    pub pane_id: u64,
    /// Length of the raw bytes that follow the message on the connection.
    /// Kept out of the JSON so terminal output is never re-encoded.
    pub length: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "snake_case")]
pub enum Response {
    Spawned {
        session_id: u64,
        pane_id: u64,
        child_pid: u32,
        /// Terminal handles already duplicated into the client, where the
        /// platform cannot attach them to the message. Empty on Unix.
        #[serde(default)]
        handles: Vec<i64>,
    },
    Attached {
        pane_id: u64,
        child_pid: u32,
        /// Raw bytes following this message: the snapshot taken at detach plus
        /// everything the pane has produced since.
        replay_length: usize,
        state: serde_json::Value,
        summary: Box<BackgroundSessionSummary>,
        /// As [`Response::Spawned::handles`].
        #[serde(default)]
        handles: Vec<i64>,
    },
    /// An attach that became shared: another client holds the pane, so instead
    /// of the terminal descriptor this connection stays open and carries the
    /// pane's output and this client's input, as framed by [`Event::Output`]
    /// and [`Request::Input`].
    ///
    /// No handles are attached: the connection *is* the terminal. The raw
    /// replay bytes follow the message, exactly as with [`Response::Attached`].
    /// `columns`/`lines` are the daemon's current effective size. A client
    /// uses them as its initial viewport once its pane is laid out; they are
    /// not that client's own size report for shared-size arbitration.
    SharedAttached {
        pane_id: u64,
        child_pid: u32,
        replay_length: usize,
        state: serde_json::Value,
        summary: Box<BackgroundSessionSummary>,
        columns: u16,
        lines: u16,
    },
    /// A shared spawn has the same replay/data-plane framing as an attach, but
    /// also returns the new canonical session snapshot.
    SharedSpawned {
        session_id: u64,
        pane_id: u64,
        child_pid: u32,
        replay_length: usize,
        state: serde_json::Value,
        summary: Box<BackgroundSessionSummary>,
        shared_state: SharedSessionState,
    },
    SharedBatchSpawned {
        mappings: Vec<SharedDraftMapping>,
        state: SharedSessionState,
    },
    /// A headless shared session was committed without attaching any client.
    SharedCreated {
        session_id: u64,
        state: SharedSessionState,
    },
    SharedOperationApplied {
        state: SharedSessionState,
    },
    SharedSnapshot {
        state: SharedSessionState,
    },
    SharedConflict {
        state: SharedSessionState,
    },
    Detached,
    Resumed {
        session_id: u64,
    },
    Sessions {
        sessions: Vec<BackgroundSessionSummary>,
        #[serde(default)]
        restorable: Vec<RestorableSessionRecord>,
    },
    /// Answers [`Request::PaneStates`], in the order asked.
    PaneStates {
        panes: Vec<PaneStateReport>,
    },
    /// Answers [`Request::StoreImage`] with the path visible to the session's
    /// child process.
    ImageStored {
        path: String,
    },
    Ok,
    /// Asks the client to prove it is the process its envelope named, before a
    /// request that would act on a protected session someone else may own is
    /// dispatched. Sent only where the platform has no peer credentials.
    ///
    /// `handle` is a pipe holding a nonce, already duplicated into the named
    /// process; the client reads it and answers with [`Request::Attested`].
    AttestationRequired {
        handle: i64,
    },
    /// The session exists but is protected and no secret was offered.
    AuthenticationRequired,
    /// The secret was wrong, or the session is inside its backoff window.
    /// Deliberately one answer for both, so the window cannot be probed.
    AuthenticationFailed,
    Error {
        message: String,
    },
}

/// Sent by the daemon to every subscriber, unprompted.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// A pane's process ended. Carries the raw `waitpid` status so a client
    /// holding the terminal can report the same exit it would have observed
    /// had it spawned the process itself.
    PaneExited {
        session_id: u64,
        pane_id: u64,
        raw_status: Option<i32>,
        /// Whether any attached client typed into this pane. Only the shared
        /// data plane can know: the daemon is the one that receives shared
        /// clients' input, and it reports here what no single client could see
        /// by itself. Exclusive clients type through their own descriptor, so
        /// for them this stays `false` and the client's own keystrokes are the
        /// truth.
        #[serde(default)]
        input_sent: bool,
    },
    /// This client is the only viewer left on a shared pane, so it may take the
    /// terminal back and stop being relayed to.
    ///
    /// An offer, not an instruction: a client that cannot take a pty — or does not
    /// want to — ignores it and stays shared. Answered with
    /// [`Request::TakeExclusive`].
    Grant { session_id: u64, pane_id: u64 },
    /// The multiplexer is about to replace itself, so this subscription is
    /// about to end for a reason that is not a failure.
    ///
    /// The sessions, the terminals and the shells all survive; only the
    /// connections do, because an `execv` cannot carry them. Announcing it is
    /// what lets a client tell an orderly replacement from a daemon that died,
    /// and reconnect promptly instead of waiting out a backoff.
    ///
    /// A client that never sees this — because the daemon crashed, or because
    /// the event lost the race with the exec — must still recover, so this is an
    /// optimisation rather than the mechanism. The mechanism is that losing the
    /// subscription is never on its own treated as a pane exiting.
    Replacing,
    /// The client holding this pane's terminal must hand it over: another
    /// client attached, and the pane is becoming shared.
    ///
    /// Delivered on the subscription connection, the only long-lived one a
    /// client keeps. The holder stops reading the pane, sends
    /// [`Request::Snapshot`] with the screen it was showing, and re-attaches —
    /// which answers with [`Response::SharedAttached`] and keeps the new
    /// connection as the shared data plane.
    Revoke { session_id: u64, pane_id: u64 },
    /// Output from a shared pane, on a shared connection. The raw bytes follow
    /// the message.
    Output {
        pane_id: u64,
        /// Length of the raw bytes that follow the message on the connection.
        length: usize,
    },
    /// The size every shared client must show this pane at: the smallest size
    /// any measured client asked for, applied by the multiplexer. Broadcast to
    /// every shared client whenever the smallest changes, and also to a client
    /// whose larger report needs correction back to the effective size.
    Size {
        session_id: u64,
        pane_id: u64,
        revision: SessionRevision,
        columns: u16,
        lines: u16,
    },
    /// Complete canonical state after a successful shared mutation. Sending a
    /// snapshot rather than a delta makes reconnect and event-gap recovery
    /// deterministic.
    SharedSessionUpdated { state: SharedSessionState },
    /// A new daemon-owned pane was added to a shared session. The pane's data
    /// connection is deliberately opened by each viewer after receiving this
    /// event, so the subscription never has to carry raw replay bytes.
    SharedPaneAdded {
        session_id: u64,
        pane_id: u64,
        child_pid: u32,
        state: SharedSessionState,
    },
    /// A pane was removed globally from a shared session. The accompanying
    /// snapshot is authoritative for layout and focus.
    SharedPaneRemoved {
        session_id: u64,
        pane_id: u64,
        state: SharedSessionState,
    },
    /// One atomic pane-set change and the exact canonical snapshot committed
    /// with it. Added and removed IDs are grouped so no viewer observes a
    /// partially applied template.
    SharedPanesChanged {
        session_id: u64,
        added: Vec<u64>,
        removed: Vec<u64>,
        state: SharedSessionState,
    },
    /// A shared data connection failed. This is recoverable; the client should
    /// replace the stream after asking for the authoritative snapshot.
    SharedStreamFailed { session_id: u64, pane_id: u64 },
    /// The shared data connection was retired cleanly after its final queued
    /// output. This is the handover/leave signal; unlike a relay failure it is
    /// an ordinary end of stream and must not trigger reconnect machinery.
    SharedClosed { session_id: u64, pane_id: u64 },
}

#[cfg(test)]
#[path = "tests/messages.rs"]
mod tests;
