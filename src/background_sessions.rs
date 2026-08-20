//! The application's half of background sessions.
//!
//! The session catalog schema, the Argon2id verifier, the secret type and the
//! catalog publisher live in the `zmux` crate, which has no GPUI or terminal
//! dependency and is shared with the multiplexer binary. What remains here is
//! the part that only makes sense inside the application: the runner holding
//! detached tabs, and the conversion from a terminal's exit event into the
//! sanitized metadata the catalog publishes.

use std::{path::PathBuf, time::Instant};

use anyhow::Result;
use terminal::{TerminalExitReason, TerminalExitSource, TerminalExited};

pub(crate) use zmux::auth::{SessionAuthentication, SessionSecret, VerifiedSession};
pub(crate) use zmux::catalog::{
    application_from_command_line, create_private_dir, read_session_catalogs,
};
pub(crate) use zmux::protocol::{
    BackgroundPaneExit, BackgroundPaneExitReason, BackgroundPaneExitSource, BackgroundPaneLayout,
    BackgroundPaneState, BackgroundPaneSummary, BackgroundSessionCatalog, BackgroundSessionSummary,
};

use zmux::catalog::SessionCatalogPublisher;

/// Sentinel runner ID used for encrypted disk records, which have no live
/// process catalog until the client resumes them.
#[cfg(feature = "session-persistence")]
pub(crate) const RESTORABLE_RUNNER_ID: u64 = u64::MAX;

/// Sanitized exit metadata for a pane the application decided to retain.
///
/// The predicate on the foreground command lives in `zmux` so the daemon
/// applies the same rule; the mapping from the terminal's own exit types lives
/// here because those types belong to the terminal crate.
pub(crate) fn background_pane_exit_from_terminal(
    exit: &TerminalExited,
) -> Option<BackgroundPaneExit> {
    let reason = exit.unexpected_reason()?;
    Some(BackgroundPaneExit {
        source: match exit.source {
            TerminalExitSource::Child => BackgroundPaneExitSource::Child,
            TerminalExitSource::StatusUnavailable => BackgroundPaneExitSource::StatusUnavailable,
            TerminalExitSource::WatcherDisconnected => {
                BackgroundPaneExitSource::WatcherDisconnected
            }
            TerminalExitSource::BackendShutdown => BackgroundPaneExitSource::BackendShutdown,
        },
        reason: match reason {
            TerminalExitReason::StatusUnavailable => BackgroundPaneExitReason::StatusUnavailable,
            TerminalExitReason::WatcherDisconnected => {
                BackgroundPaneExitReason::WatcherDisconnected
            }
            TerminalExitReason::BackendShutdown => BackgroundPaneExitReason::BackendShutdown,
            TerminalExitReason::ExitedBeforeInput => BackgroundPaneExitReason::ExitedBeforeInput,
            TerminalExitReason::ForegroundCommand => BackgroundPaneExitReason::ForegroundCommand,
        },
        exit_code: exit.exit_code,
        child_pid: exit.child_pid,
        input_sent: exit.input_sent,
        foreground_is_shell: exit.foreground_is_shell,
        foreground_command: exit
            .foreground_command
            .as_deref()
            .filter(|command| BackgroundPaneExit::foreground_command_is_publishable(command))
            .map(ToOwned::to_owned),
    })
}

/// Owns sessions that are not currently attached to a terminal view.
///
/// This deliberately has no GPUI or platform dependency. A future local daemon or
/// remote transport can own the same runner without also owning window state.
pub(crate) struct BackgroundSessionRunner<T> {
    sessions: Vec<DetachedSession<T>>,
    catalog: SessionCatalogPublisher,
}

struct DetachedSession<T> {
    value: T,
    authentication: Option<SessionAuthentication>,
    failed_authentications: u32,
    refuse_until: Option<Instant>,
}

impl<T> Default for BackgroundSessionRunner<T> {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            catalog: SessionCatalogPublisher::new(&session_catalog_dir()),
        }
    }
}

impl<T> BackgroundSessionRunner<T> {
    pub(crate) fn runner_id(&self) -> u64 {
        self.catalog.runner_id()
    }

    pub(crate) fn detach(&mut self, session: T, authentication: Option<SessionAuthentication>) {
        self.sessions.push(DetachedSession {
            value: session,
            authentication,
            failed_authentications: 0,
            refuse_until: None,
        });
    }

    /// Whether this session is inside its backoff window and must refuse a
    /// reconnect attempt without evaluating the secret.
    pub(crate) fn authentication_is_refused_at(&self, index: usize) -> bool {
        self.sessions
            .get(index)
            .and_then(|session| session.refuse_until)
            .is_some_and(|until| Instant::now() < until)
    }

    /// Records a wrong secret and opens the next backoff window.
    ///
    /// Only called for attempts that were actually evaluated. Attempts already
    /// refused by the window do not extend it, so someone retrying too eagerly
    /// cannot drive their own lockout upward.
    pub(crate) fn record_failed_authentication_at(&mut self, index: usize) {
        if let Some(session) = self.sessions.get_mut(index) {
            session.failed_authentications = session.failed_authentications.saturating_add(1);
            session.refuse_until = Instant::now().checked_add(
                zmux::auth::failed_authentication_delay(session.failed_authentications),
            );
        }
    }

    pub(crate) fn clear_failed_authentications_at(&mut self, index: usize) {
        if let Some(session) = self.sessions.get_mut(index) {
            session.failed_authentications = 0;
            session.refuse_until = None;
        }
    }

    pub(crate) fn reconnect_at(&mut self, index: usize) -> Option<T> {
        (index < self.sessions.len()).then(|| self.sessions.remove(index).value)
    }

    pub(crate) fn authentication_at(&self, index: usize) -> Option<&SessionAuthentication> {
        self.sessions.get(index)?.authentication.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.sessions.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.sessions.iter().map(|session| &session.value)
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.sessions.iter_mut().map(|session| &mut session.value)
    }

    /// Sessions that reattach without a secret.
    ///
    /// Process control requests are authenticated only by the endpoint token,
    /// and that token sits in a file which every process running as this user
    /// can read. Anything reachable from the control socket must therefore go
    /// through these iterators rather than [`Self::iter`]/[`Self::iter_mut`]:
    /// otherwise the token alone would reveal that a protected session exists
    /// and let its state be modified, which is exactly what holding a secret is
    /// supposed to prevent.
    pub(crate) fn iter_unprotected(&self) -> impl Iterator<Item = &T> {
        self.sessions
            .iter()
            .filter(|session| session.authentication.is_none())
            .map(|session| &session.value)
    }

    pub(crate) fn iter_unprotected_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.sessions
            .iter_mut()
            .filter(|session| session.authentication.is_none())
            .map(|session| &mut session.value)
    }

    pub(crate) fn publish(&mut self, sessions: Vec<BackgroundSessionSummary>) -> Result<()> {
        self.catalog.publish_sessions(sessions)
    }
}

pub(crate) fn session_catalog_dir() -> PathBuf {
    zmux::paths::session_catalog_dir()
}

/// Whether a published session's process is a Zetta window rather than the
/// multiplexer, told apart by whether it has a Zetta control endpoint.
///
/// The multiplexer daemon holds sessions too, and its catalog is published
/// under *its* process. Only Zetta processes write a `control-{process_id}.json`
/// endpoint, so its presence is what separates in-process fallback sessions from
/// sessions the multiplexer is holding — the reconnect paths must not conflate
/// the two, or a session another window kept because the multiplexer was
/// unreachable gets routed to the daemon, which does not have it.
pub(crate) fn process_is_zetta(process_id: u32) -> bool {
    session_catalog_dir()
        .join(format!("control-{process_id}.json"))
        .is_file()
}

/// The sessions a set of published catalogs attributes to the multiplexer.
///
/// The multiplexer is not a Zetta process, so a catalog counts only when its
/// process has no Zetta control endpoint. A Zetta process that kept a session
/// in memory because the multiplexer was unreachable publishes a catalog under
/// its own process too, and those sessions are not the multiplexer's to attach —
/// routing them there is how a reconnect that used to transfer the tab in
/// process turned into "could not attach that session".
///
/// Sessions scoped to another Zetta process are left out. A private backgrounded
/// tab keeps its session to the window that did it, so another process must not
/// be offered it: the multiplexer refuses that attach, and listing it anyway
/// would put an entry in the picker whose only outcome is an error. A session
/// with no scope is shared, and one scoped to this process is this window's own.
///
/// The predicate and the process id are parameters so the discrimination is
/// testable without touching the real session directory.
pub(crate) fn multiplexer_held_catalog_sessions(
    catalogs: &[BackgroundSessionCatalog],
    is_zetta: impl Fn(u32) -> bool,
    this_process: u32,
) -> impl Iterator<Item = (&BackgroundSessionCatalog, &BackgroundSessionSummary)> {
    catalogs
        .iter()
        .filter(move |catalog| !is_zetta(catalog.process_id))
        .flat_map(|catalog| {
            catalog
                .sessions
                .iter()
                .map(move |session| (catalog, session))
        })
        .filter(move |(_, session)| {
            session
                .scoped_to
                .is_none_or(|process_id| process_id == this_process)
        })
}

#[cfg(test)]
#[path = "tests/background_sessions.rs"]
mod tests;
