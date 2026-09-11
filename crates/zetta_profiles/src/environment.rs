//! The environment every Zetta terminal is started with.
//!
//! Kept here because two different processes create Zetta's ptys. The
//! application creates one when it opens a pane itself; the daemon creates one
//! when a pane is added to a shared session, and a daemon is a background
//! process with no `TERM` of its own. A pane spawned by the daemon without this
//! inherited that: the shell decided the terminal had no colour, and drew a
//! monochrome prompt beside identical panes that were in colour.

/// What the caller knows that this crate does not.
#[derive(Clone, Copy, Debug)]
pub struct TerminalEnvironmentOptions<'a> {
    /// Zetta's version, reported as `TERM_PROGRAM_VERSION`.
    pub version: &'a str,
}

/// The variables a Zetta pty is given, in addition to whatever it inherits.
///
/// `TERM` is fixed rather than probed: it describes what Zetta's terminal
/// emulator implements, not what the machine starting it happens to have in
/// its own environment.
pub fn terminal_environment(options: TerminalEnvironmentOptions<'_>) -> Vec<(String, String)> {
    vec![
        ("ZETTA_TERM".to_owned(), "true".to_owned()),
        ("TERM_PROGRAM".to_owned(), "zetta".to_owned()),
        ("TERM".to_owned(), "xterm-256color".to_owned()),
        ("COLORTERM".to_owned(), "truecolor".to_owned()),
        (
            "TERM_PROGRAM_VERSION".to_owned(),
            options.version.to_owned(),
        ),
    ]
}

/// Names that must not be carried over from whatever started the pty.
///
/// `ZED_TERM` is the upstream marker, which a shell integration would take as
/// evidence it is running inside Zed. `SHLVL` is removed so the spawned shell
/// initializes it to 1, matching what a standalone terminal emulator does
/// rather than counting the process that started the daemon.
pub const REMOVED_TERMINAL_ENVIRONMENT: &[&str] = &["ZED_TERM", "SHLVL"];

/// The locale a pty falls back to when neither the caller nor the machine set
/// one. A GUI application launched from Finder has no `LANG` at all, and
/// neither does a daemon started by one.
pub const FALLBACK_LANG: &str = "en_US.UTF-8";
