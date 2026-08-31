//! The session catalog schema.
//!
//! These types are the published description of what the multiplexer is
//! holding. They carry no terminal output, environment values or full command
//! lines, and a protected session is reduced to an ID and a flag before it is
//! ever written — see [`BackgroundSessionSummary::for_public_catalog`].

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The schema version written into every published catalog.
///
/// A catalog carrying any other version is skipped, and removed once the process
/// that published it is gone. That is what becomes of the versions up to 4 that
/// Zetta published from its own process before sessions moved into the
/// multiplexer: same directory, same `zetta-PROCESS-GENERATION.json` name.
pub const CATALOG_VERSION: u32 = 1;

/// A disk-retained session record before its encrypted payload has been
/// opened. These fields are deliberately opaque: titles, commands, working
/// directories, layout, and protected-session details stay inside the age
/// ciphertext.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestorableSessionRecord {
    pub id: u64,
    pub created_at: u64,
    pub updated_at: u64,
    pub metadata_bytes: u64,
    pub snapshot_bytes: u64,
    pub scrollback_bytes: u64,
    pub protected: bool,
    /// Whether the protection is a key sealed to the configured age recipients
    /// rather than a secret someone typed.
    ///
    /// The envelope itself is inside the ciphertext, and the decision of whether
    /// to ask a person for anything has to be made before the record is opened —
    /// so unlike a live session, where the envelope travels in the catalog and
    /// its presence is the whole marker, a record needs this flag. It discloses
    /// no more than `protected` beside it already does.
    #[serde(default)]
    pub auto_protected: bool,
    pub restorable: bool,
}

/// Version of the local Zetta process-control protocol used to open a
/// multiplexer-held session in a window, including the explicit `new_window`
/// request and the two-phase `run_wait`/`run_complete` exchange.
pub const CONTROL_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundSessionCatalog {
    pub version: u32,
    pub process_id: u32,
    pub runner_id: u64,
    pub sessions: Vec<BackgroundSessionSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundSessionSummary {
    pub id: u64,
    pub title: String,
    pub authentication_required: bool,
    pub active_pane: u64,
    pub layout: BackgroundPaneLayout,
    pub panes: Vec<BackgroundPaneSummary>,
    /// `true` if the session is currently exclusively attached by another
    /// client. A reconnect will trigger a revoke→shared handoff.
    #[serde(default)]
    pub held: bool,
    /// The Zetta process this session is scoped to, or `None` if it is shared.
    ///
    /// Plain backgrounding is private: the session belongs to the window that
    /// put it away, and another Zetta process must neither offer it in a picker
    /// nor be able to attach it. Sharing — including keep-running's default —
    /// is what makes it everyone's, and clears this.
    ///
    /// Carried in the catalog because the catalog is one file that every
    /// process reads, so a reader has to be able to tell which entries are its
    /// own. The multiplexer refuses the attach either way; this is what keeps a
    /// session that cannot be attached from being offered in the first place.
    #[serde(default)]
    pub scoped_to: Option<u32>,
    /// The session key sealed to the configured age recipients, when this
    /// session was protected automatically rather than with a typed secret.
    ///
    /// Published deliberately. It is an age v1 file — public ciphertext — so the
    /// only party it helps is one holding a matching private key, which is the
    /// party being let in. Carrying it here rather than keeping it in the
    /// daemon's memory is what lets a session outlive the daemon: a key nobody
    /// can recover afterwards would take its session with it.
    ///
    /// Its presence is what marks a session as automatically protected; there is
    /// no separate flag to keep in step with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_envelope: Option<String>,
}

impl BackgroundSessionSummary {
    /// Strips everything a protected session must not reveal while detached.
    /// Applied on the publishing side, so no caller can forget it.
    ///
    /// The key envelope survives this on purpose: a protected session is exactly
    /// when it is needed, and it reveals nothing to a reader who cannot open it.
    pub(crate) fn for_public_catalog(mut self) -> Self {
        if self.authentication_required {
            self.title = "Protected session".to_owned();
            self.active_pane = 0;
            self.layout = BackgroundPaneLayout::Pane { pane_id: 0 };
            self.panes.clear();
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BackgroundPaneLayout {
    Pane {
        pane_id: u64,
    },
    Split {
        axis: String,
        first: Box<BackgroundPaneLayout>,
        second: Box<BackgroundPaneLayout>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundPaneSummary {
    pub id: u64,
    pub label: String,
    pub profile: String,
    pub configured_command: String,
    pub application: String,
    pub foreground_command: Option<Vec<String>>,
    pub terminal_title: Option<String>,
    pub working_directory: Option<PathBuf>,
    pub state: BackgroundPaneState,
    #[serde(default)]
    pub exit: Option<BackgroundPaneExit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundPaneState {
    Starting,
    Running,
    Exited,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundPaneExitSource {
    Child,
    StatusUnavailable,
    WatcherDisconnected,
    BackendShutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundPaneExitReason {
    StatusUnavailable,
    WatcherDisconnected,
    BackendShutdown,
    ExitedBeforeInput,
    ForegroundCommand,
}

/// Sanitized exit metadata retained with a failed pane. It intentionally
/// contains no terminal output, environment values, working directory, or
/// full command line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundPaneExit {
    pub source: BackgroundPaneExitSource,
    pub reason: BackgroundPaneExitReason,
    pub exit_code: Option<i32>,
    pub child_pid: Option<u32>,
    pub input_sent: bool,
    pub foreground_is_shell: Option<bool>,
    pub foreground_command: Option<String>,
}

impl BackgroundPaneExit {
    /// Whether a foreground command name is safe to retain: short, and drawn
    /// from a character set that cannot smuggle arguments or a secret into the
    /// catalog. The conversion from a terminal exit lives in the application,
    /// which owns the terminal types; this is the shared predicate it applies.
    pub fn foreground_command_is_publishable(command: &str) -> bool {
        !command.is_empty()
            && command.len() <= 64
            && command.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
    }

    /// The one-line heading a retained pane is shown under.
    ///
    /// Losing contact with the multiplexer is not the terminal exiting: the
    /// process may well still be running, and the session may still be
    /// attachable from another window. Saying "exited unexpectedly" for it
    /// conflates a broken control channel with a shell that died, which is
    /// exactly the confusion that made a spurious disconnect hard to diagnose.
    pub fn heading(&self) -> &'static str {
        match self.reason {
            BackgroundPaneExitReason::WatcherDisconnected => {
                "Lost contact with the session multiplexer"
            }
            _ => "Terminal exited unexpectedly",
        }
    }

    pub fn reason_text(&self) -> String {
        let mut text = match self.reason {
            BackgroundPaneExitReason::StatusUnavailable => {
                "the child exited but its exit status was unavailable".to_owned()
            }
            BackgroundPaneExitReason::WatcherDisconnected => {
                "the multiplexer stopped reporting this terminal's process, so its exit status \
                 cannot be observed here. The process may still be running; the session can be \
                 reattached once the multiplexer is reachable again"
                    .to_owned()
            }
            BackgroundPaneExitReason::BackendShutdown => {
                "the terminal backend shut down unexpectedly".to_owned()
            }
            BackgroundPaneExitReason::ExitedBeforeInput => {
                "the shell exited before receiving user input".to_owned()
            }
            BackgroundPaneExitReason::ForegroundCommand => self
                .foreground_command
                .as_deref()
                .map(|command| format!("the shell exited while {command:?} was foreground"))
                .unwrap_or_else(|| "the shell exited while a command was foreground".to_owned()),
        };
        if let Some(code) = self.exit_code {
            text.push_str(&format!(" (exit code {code})"));
        }
        if let Some(pid) = self.child_pid {
            text.push_str(&format!(" [child PID {pid}]"));
        }
        text
    }
}

impl std::fmt::Display for BackgroundPaneState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Failed => "failed",
        };
        formatter.write_str(state)
    }
}
