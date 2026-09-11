//! What a Zetta profile runs, and the environment it runs in.
//!
//! Both the application and `zmux` need to turn a profile *name* into a command
//! on the machine that will execute it. The application needs it to start a
//! local terminal; the daemon needs it because a pane in a shared session is
//! created on the daemon's host and the name is all that crosses the wire. A
//! viewer on another machine cannot resolve it — its `$SHELL` is its own, and
//! shipping its environment put a Linux `PATH` in front of a macOS shell.
//!
//! So this crate holds the half of Zetta's configuration that answers "what
//! does this profile run here": the shell discovery, the profile entries of the
//! configuration file, and the environment every Zetta terminal is given. It
//! deliberately knows nothing about icons, themes, or anything else that only
//! matters to something with a window — that half stays in the application's
//! `config` module, which layers it back on.

use std::path::Path;

mod config_file;
mod discovery;
mod environment;
mod shell_integration;

pub use discovery::discovered_profiles;
pub use environment::{
    FALLBACK_LANG, REMOVED_TERMINAL_ENVIRONMENT, TerminalEnvironmentOptions, terminal_environment,
};
pub use shell_integration::{ShellKind, runs_a_command, shell_integration_startup_command};

/// What a profile starts, on the machine that will start it.
///
/// `program: None` is the host's login shell — the same meaning
/// `util::shell::Shell::System` carries in the application, spelled without
/// depending on it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProfileCommand {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

impl ProfileCommand {
    pub fn system() -> Self {
        Self::default()
    }

    pub fn program(program: impl Into<String>) -> Self {
        Self {
            program: Some(program.into()),
            args: Vec::new(),
        }
    }

    pub fn with_args(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: Some(program.into()),
            args,
        }
    }

    /// The shell family this command belongs to, for the shell integration and
    /// the startup handshake. The host's login shell is resolved through
    /// `SHELL`, because that is what it will actually start.
    pub fn shell_kind(&self) -> ShellKind {
        ShellKind::of(self.program.as_deref().unwrap_or(&system_shell()))
    }
}

/// A profile as this host can run it. The application's own `Profile` is this
/// plus what it takes to draw one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileDefinition {
    pub name: String,
    pub command: ProfileCommand,
}

/// The login shell of the user this process runs as.
///
/// `SHELL` first, because that is what a terminal emulator starting an
/// interactive shell is expected to honour, then the portable fallback.
pub fn system_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(windows) {
            "powershell.exe".to_owned()
        } else {
            "/bin/sh".to_owned()
        }
    })
}

/// Every profile this host offers: the shells that are installed, overlaid with
/// the profiles the configuration file names.
///
/// `config_path` is the `config.json` to read. A file that is missing or
/// unreadable leaves the discovered set, which is the same thing the
/// application does — a broken configuration file must not leave a host unable
/// to open a shell at all.
pub fn profiles(config_path: &Path) -> Vec<ProfileDefinition> {
    let mut profiles = discovered_profiles();
    match config_file::configured_profiles(config_path) {
        Ok(configured) => config_file::merge(&mut profiles, configured),
        Err(error) => log_read_failure(config_path, &error),
    }
    profiles
}

/// Resolves a profile name to what it runs here, case-insensitively the way the
/// application matches profile names everywhere else.
pub fn resolve(config_path: &Path, name: &str) -> Option<ProfileCommand> {
    profiles(config_path)
        .into_iter()
        .find(|profile| profile.name.eq_ignore_ascii_case(name))
        .map(|profile| profile.command)
}

fn log_read_failure(config_path: &Path, error: &anyhow::Error) {
    // No `log` dependency: this crate is linked into the daemon, the
    // application, and the terminal, and only one of those installs a logger.
    // A configuration file that cannot be read is reported where it is opened;
    // here it only decides whether the discovered set stands alone.
    let _ = (config_path, error);
}

#[cfg(test)]
#[path = "tests/lib.rs"]
mod tests;
