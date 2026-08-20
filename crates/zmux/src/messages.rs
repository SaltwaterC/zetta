//! The client/daemon wire protocol.
//!
//! Deliberately transport-agnostic: nothing here depends on descriptor passing
//! or on peer credentials, so the same messages can later carry a session over
//! a remote transport. Descriptor passing is an optimisation the local
//! transport applies to [`Response::Spawned`] and [`Response::Attached`].

use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::protocol::{BackgroundSessionSummary, RestorableSessionRecord};

/// The wire format, and what a client and a multiplexer compare before they
/// trust each other to understand one another.
///
/// Pinned at 2 while Zetta is under development: the protocol is not stabilised,
/// so its shape changes freely and a numbered history of every change would be
/// bookkeeping about versions nobody is running. What that costs is the guard —
/// while the number stays put, two builds whose messages disagree both believe
/// they are compatible, and the failure is whatever the mismatch produces rather
/// than a version error. So after changing a message's shape, replace the running
/// multiplexer (`zmux --upgrade`) or stop it (`zmux stop`); a stale one is the
/// only way a mismatch can arise on one machine.
///
/// When the protocol does stabilise, this becomes what it says: bumped whenever a
/// message's shape changes, so a client and a daemon that cannot parse each other
/// say so instead of failing obscurely.
pub const PROTOCOL_VERSION: u32 = 2;

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
    pub request: Request,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum Request {
    /// Starts a process under the multiplexer and hands its terminal back.
    Spawn(SpawnRequest),
    /// Takes over a pane's terminal from the multiplexer.
    ///
    /// `pane_id` is absent for the session's first pane, which is how an
    /// attach starts: the caller cannot know a protected session's pane
    /// identifiers, because they are not published until it has authenticated.
    Attach {
        session_id: u64,
        pane_id: Option<u64>,
        secret: Option<String>,
    },
    /// The revoke handshake's answer: the client holding a pane's terminal
    /// stopped reading it and is giving back the screen it was showing, so the
    /// multiplexer can resume reading and relay the pane to every client that
    /// attaches.
    ///
    /// Sent over a fresh connection from the *revoked* client, after
    /// [`Event::Revoke`]. The raw snapshot bytes follow the message. The
    /// connection carries nothing else and closes afterwards.
    ///
    /// `columns`/`lines` are the size the holder was showing the pane at:
    /// shared clients join at that size until their own reports refine it,
    /// which matters because an exclusive client resizes through its own
    /// descriptor and the multiplexer would otherwise never learn the size.
    Snapshot {
        session_id: u64,
        pane_id: u64,
        /// Length of the raw bytes that follow the message on the connection.
        length: usize,
        columns: u16,
        lines: u16,
    },
    /// Input from a shared client, on the shared connection [`Request::Attach`]
    /// left open after it was answered with [`Response::SharedAttached`].
    ///
    /// The raw bytes follow the message.
    Input {
        /// Length of the raw bytes that follow the message on the connection.
        length: usize,
    },
    /// Gives a session back to the multiplexer to hold. The client has already
    /// stopped reading the panes' terminals by the time this is sent.
    Detach(DetachRequest),
    /// Restores an encrypted disk record after the client has decrypted it.
    /// The daemon receives state and authentication metadata, never an age
    /// identity or a private key.
    Resume(ResumeRequest),
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
        columns: u16,
        lines: u16,
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
    SetSessionScope {
        session_id: u64,
        shared: bool,
        /// The Argon2id verifier for the secret a joining process must present.
        ///
        /// Required when sharing a session that has none: a session another
        /// process can join unchallenged hands it whatever its terminals can
        /// already do, which for a shell that has answered `sudo` is root.
        verifier: Option<String>,
    },
    /// Removes a session from the catalog without killing it. The session
    /// continues running under the daemon but is no longer listed or
    /// attachable until the daemon restarts (at which point it is gone).
    Forget { session_id: u64 },
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
    /// Releases a pane whose window closed while its process was still running.
    ///
    /// Dropping the client's descriptor is not enough to tell the multiplexer
    /// anything: it holds its own, so the pane stays marked as taken, nobody
    /// drains it, and the program blocks as soon as the terminal's buffer
    /// fills. This hands the pane back so it is either drained or, if the
    /// session was never meant to outlive its window, ended.
    ClosePane { session_id: u64, pane_id: u64 },
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
    pub verifier: Option<String>,
    pub failed_authentications: u32,
    pub backoff_seconds: u64,
    pub created_at: u64,
    pub updated_at: u64,
    /// The session secret is sent only after the client has decrypted the
    /// record. It is checked by the daemon and then discarded before the
    /// restored record is kept in memory.
    pub secret: Option<String>,
    pub snapshots: Vec<ResumeSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeSnapshot {
    pub pane_id: u64,
    pub bytes: Vec<u8>,
}

/// Publishes a session the client is still showing, so other clients can find
/// and attach to it.
///
/// Carries the same summary and state a detach does, and for the same reason:
/// the catalog needs something to list, and a client that joins rebuilds its tab
/// from the state. It carries no snapshots, because the panes are still being
/// read by the sharing client — the screen a joining client starts from comes
/// from the revoke handover, which is where the holder is asked for it.
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
    /// `columns`/`lines` are the size every shared client shows the pane at,
    /// which the client applies before reporting its own over `Resize`.
    SharedAttached {
        pane_id: u64,
        child_pid: u32,
        replay_length: usize,
        state: serde_json::Value,
        summary: Box<BackgroundSessionSummary>,
        columns: u16,
        lines: u16,
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
    Ok,
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
    /// any of them asked for, applied by the multiplexer. Broadcast to every
    /// shared client whenever the smallest changes.
    Size {
        session_id: u64,
        pane_id: u64,
        columns: u16,
        lines: u16,
    },
}

#[cfg(test)]
#[path = "tests/messages.rs"]
mod tests;
