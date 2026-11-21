//! Talking to the multiplexer.
//!
//! Each request opens its own connection, which keeps request and response
//! trivially paired. Asynchronous events — a pane's process exiting — arrive on
//! one long-lived subscription instead, because they are not answers to
//! anything.

use std::{
    collections::{HashMap, VecDeque},
    io,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use alacritty_terminal::tty::ConsolePalette;
use anyhow::{Context as _, Result};

#[cfg(unix)]
use std::os::fd::OwnedFd;

/// What a received message may carry alongside it: terminal descriptors on
/// Unix, and nothing on Windows, where handles travel inside the message
/// already duplicated into this process.
#[cfg(unix)]
type Descriptors = Vec<OwnedFd>;
#[cfg(windows)]
type Descriptors = Vec<()>;

use crate::{
    messages::{
        CreateSharedRequest, DetachRequest, Envelope, Event, MAX_IMAGE_BYTES, PROTOCOL_VERSION,
        PaneSnapshot, PaneStateReport, Request, Response, SessionRevision, SharedOperationId,
        SharedSessionOperation, SharedSessionOperationRequest, SharedSessionState,
        SharedSpawnBatchRequest, SharedSpawnRequest, SpawnRequest,
    },
    paths::session_catalog_dir,
    protocol::{BackgroundSessionSummary, RestorableSessionRecord},
    retention::Retention,
    server::endpoint_path,
    transport::{Connection, Endpoint, Stream},
};

#[cfg(feature = "session-persistence")]
use crate::messages::{ResumeRequest, ResumeSnapshot};

#[cfg(feature = "session-persistence")]
use crate::persistence::{DaemonOptionsFile, PersistenceOptions};

// Unconditional: what protects a session is a verifier and, when it was sealed
// rather than typed, an envelope this crate only ever passes along. Neither
// needs age, so detach, share and scope carry them in every build.
use crate::auth::SessionAuthentication;
use crate::auth::SessionSecret;
use crate::messages::ClientId;
use crate::remote::{RemoteTarget, RemoteTransport};
#[cfg(feature = "session-persistence")]
use crate::secret_prompt;

mod configure;
mod connect;
mod daemon_start;
mod exit_reports;
mod requests;
mod sessions;
mod shared_ops;
mod shared_pane;
mod subscription;

pub(crate) use daemon_start::*;
pub use exit_reports::*;
pub use shared_pane::*;
pub use subscription::*;

/// How long to wait for a daemon this process just started to publish its
/// endpoint. Generous, because the first start also creates the directory.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const STARTUP_POLL: Duration = Duration::from_millis(10);
/// A request must not be able to leave the terminal-opening task waiting on a
/// daemon forever. This also bounds a peer that accepted a connection but does
/// not understand the request framing.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
pub struct Client {
    /// The endpoint can change while this client is alive: an in-place daemon
    /// replacement publishes a fresh token and socket.  Keeping it behind a
    /// mutex lets every pane-opening task share the repaired value without
    /// replacing the client (and, importantly, its subscription registries).
    endpoint: Mutex<Endpoint>,
    /// Where this client found its multiplexer, so a subscription that is lost
    /// can look again. Re-reading the endpoint file rather than reusing the
    /// `Endpoint` matters: a replacement publishes its own process id, and one
    /// day may publish a different socket.
    directory: PathBuf,
    /// Present for a client that reaches a remote daemon through one
    /// persistent stream-local SSH forward. The remote endpoint is refreshed
    /// through this transport rather than read from the local session
    /// directory.
    remote: Option<Arc<RemoteTransport>>,
    /// Stable across request connections and subscription reconnects.
    client_id: ClientId,
    /// Remote clients are deliberately restricted to the shared byte-stream
    /// protocol. Local clients retain descriptor handover and PID ownership.
    stream_only: bool,
    /// An authenticated remote session key used by later control requests and
    /// subscription reconciliation. It is shared by request clones and is
    /// zeroized when the logical client is dropped.
    session_secret: Arc<Mutex<Option<SessionSecret>>>,
    operation_counter: Arc<AtomicU64>,
}

/// The result of applying a retention policy to a daemon.
///
/// `requested_retention` remains the user's choice even when a temporary
/// recipient lookup makes the daemon use `effective_retention` for now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetentionConfiguration {
    pub requested_retention: Retention,
    pub effective_retention: Retention,
    pub degraded_reason: Option<String>,
}

/// A retention policy whose recipients have been resolved, ready to be sent to
/// any number of daemons with [`Client::configure_resolved`].
///
/// Separate from sending it because resolving can be a GitHub fetch: a caller
/// with several connections to configure from one configuration should make
/// that fetch once, not once per connection.
#[cfg(feature = "session-persistence")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedRetention {
    pub requested_retention: Retention,
    pub effective_retention: Retention,
    pub degraded_reason: Option<String>,
    pub recipients: Vec<String>,
}

#[cfg(feature = "session-persistence")]
impl ResolvedRetention {
    /// Resolves `persistence`'s recipients for `retention`, degrading disk to
    /// `fallback_retention` on a temporary lookup failure exactly as
    /// [`Client::configure_with_retention_and_persistence_resilient`] does.
    pub fn resolve(
        retention: Retention,
        persistence: &PersistenceOptions,
        fallback_retention: Retention,
    ) -> Result<Self> {
        retention.validate()?;
        fallback_retention.validate()?;
        anyhow::ensure!(
            matches!(fallback_retention, Retention::Memory { .. }),
            "retention fallback must be an in-memory policy"
        );
        let (effective_retention, degraded_reason, recipients) = resolve_effective_retention(
            retention,
            &persistence.recipients,
            fallback_retention,
            crate::persistence::resolve_recipient_strings_for_startup,
        )?;
        Ok(Self {
            requested_retention: retention,
            effective_retention,
            degraded_reason,
            recipients,
        })
    }

    /// What a daemon that accepted this policy is running with.
    pub fn configuration(&self) -> RetentionConfiguration {
        RetentionConfiguration {
            requested_retention: self.requested_retention,
            effective_retention: self.effective_retention,
            degraded_reason: self.degraded_reason.clone(),
        }
    }
}

/// A connected client together with the retention policy the daemon actually
/// accepted during startup.
pub struct ConfiguredClient {
    pub client: Client,
    pub requested_retention: Retention,
    pub effective_retention: Retention,
    pub degraded_reason: Option<String>,
}

#[cfg(feature = "session-persistence")]
fn resolve_effective_retention(
    retention: Retention,
    recipient_values: &[String],
    fallback_retention: Retention,
    resolve: impl FnOnce(
        &[String],
    ) -> std::result::Result<
        Vec<String>,
        crate::persistence::RecipientResolutionError,
    >,
) -> Result<(Retention, Option<String>, Vec<String>)> {
    if !matches!(retention, Retention::Disk) {
        return Ok((retention, None, Vec::new()));
    }
    match resolve(recipient_values) {
        Ok(recipients) => Ok((retention, None, recipients)),
        Err(error) if error.is_temporary() => {
            Ok((fallback_retention, Some(format!("{error:#}")), Vec::new()))
        }
        Err(error) => Err(error.into_anyhow()),
    }
}

/// A pane's terminal, as handed over by the multiplexer.
pub struct AttachedPane {
    pub session_id: u64,
    pub pane_id: u64,
    pub child_pid: u32,
    /// Identifies this particular acquisition of the pane, not merely the
    /// process that acquired it. A delayed cleanup for an older acquisition
    /// must not release a newer one from the same process.
    attachment_client_id: ClientId,
    /// The PTY master. The holder reads and writes it directly, so an attached
    /// pane costs exactly what a locally spawned one costs.
    #[cfg(unix)]
    pub descriptor: OwnedFd,
    /// The pseudoconsole's pipes, duplicated into this process. A console
    /// handle itself cannot be shared, so resizing goes back to the owner.
    #[cfg(windows)]
    pub conout: std::os::windows::io::OwnedHandle,
    #[cfg(windows)]
    pub conin: std::os::windows::io::OwnedHandle,
    /// Output produced while the pane was detached, to be replayed into a
    /// fresh terminal before it is shown.
    pub replay: Vec<u8>,
}

impl AttachedPane {
    pub fn attachment_client_id(&self) -> &ClientId {
        &self.attachment_client_id
    }
}

fn is_invalid_token_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string() == "invalid multiplexer token")
}

fn is_transport_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<io::Error>().is_some_and(|error| {
            matches!(
                error.kind(),
                io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::BrokenPipe
                    | io::ErrorKind::NotFound
                    | io::ErrorKind::AddrNotAvailable
                    | io::ErrorKind::TimedOut
                    | io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::WouldBlock
            )
        })
    })
}

/// Whether a refusal is the multiplexer saying it does not speak this build's
/// protocol.
///
/// Unlike every other failure a client retries, this one is permanent for the
/// life of the process: no replacement is coming that this build could talk to,
/// because the replacement has already happened.
fn is_protocol_mismatch_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| crate::messages::is_protocol_mismatch_message(&cause.to_string()))
}

fn is_unsupported_configure(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let message = cause.to_string();
        message.contains("unknown variant `configure`")
            || message.contains("unknown variant 'configure'")
            || message.contains("unknown variant \"configure\"")
    })
}

/// Whether a connection insists that the multiplexer speaks this build's
/// protocol.
#[derive(Clone, Copy)]
enum VersionCheck {
    /// Every ordinary request: a client that cannot be understood must not send
    /// one, and finding that out here is what lets it fall back to a local
    /// process rather than failing to open a terminal.
    Required,
    /// `Request::Upgrade` alone, which the daemon accepts across the boundary
    /// precisely because crossing it is what the request is for.
    Tolerated,
}

pub enum AttachOutcome {
    Attached {
        pane: AttachedPane,
        state: serde_json::Value,
        summary: BackgroundSessionSummary,
    },
    /// The pane is shared: the connection stayed open and is the data plane.
    SharedAttached {
        pane: SharedPane,
        state: serde_json::Value,
        summary: BackgroundSessionSummary,
    },
    AuthenticationRequired,
    AuthenticationFailed,
}

pub struct SharedSpawnedPane {
    pub pane: SharedPane,
    pub state: SharedSessionState,
}

pub struct SharedBatchSpawned {
    pub mappings: Vec<crate::messages::SharedDraftMapping>,
    pub state: SharedSessionState,
}

pub struct SharedCreated {
    pub session_id: u64,
    pub state: SharedSessionState,
}

pub enum SharedBatchResult {
    Applied(SharedBatchSpawned),
    Conflict(SharedSessionState),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SharedOperationResult {
    Applied(SharedSessionState),
    Conflict(SharedSessionState),
}

/// The error for a response that is not the success `request` expects: the
/// daemon's own refusal when it gave one, otherwise a protocol mismatch.
fn refusal(response: Response, request: &str) -> anyhow::Error {
    match response {
        Response::Error { message } => anyhow::anyhow!("{message}"),
        other => anyhow::anyhow!("unexpected response to {request}: {other:?}"),
    }
}

#[cfg(test)]
#[path = "tests/client.rs"]
mod tests;

/// Takes ownership of the terminal the multiplexer handed over.
#[cfg(unix)]
fn claim_terminal(descriptors: &mut Vec<OwnedFd>, _handles: Vec<i64>) -> Result<OwnedFd> {
    anyhow::ensure!(
        descriptors.len() == 1,
        "the multiplexer did not hand over a terminal"
    );
    Ok(descriptors.remove(0))
}

#[cfg(windows)]
fn claim_terminal(
    _descriptors: &mut Vec<()>,
    handles: Vec<i64>,
) -> Result<(
    std::os::windows::io::OwnedHandle,
    std::os::windows::io::OwnedHandle,
)> {
    anyhow::ensure!(
        handles.len() == 2,
        "the multiplexer did not hand over a console's pipes"
    );
    let mut claimed = crate::transport::claim_duplicated(&handles);
    let conin = claimed.remove(1);
    let conout = claimed.remove(0);
    Ok((conout, conin))
}
