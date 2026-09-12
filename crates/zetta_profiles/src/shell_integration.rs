//! Loading Zetta's shell integration into an interactive shell.
//!
//! The command is sent after the shell's startup files have run, so a stale
//! `zetta` found earlier on `PATH` cannot leave the pane with CWD-only
//! tracking. It lives here rather than beside the application's spawn path
//! because the shell it has to match is resolved on the machine that starts the
//! pane — for a shared session, the daemon's host, not the viewer's.

/// The shell families Zetta can install its integration into. Anything else
/// runs without it rather than being sent a line it would print.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellKind {
    Bash,
    Zsh,
    Fish,
    PowerShell,
    Other,
}

impl ShellKind {
    /// Classifies a program by its executable basename, the way every other
    /// shell check in Zetta does.
    ///
    /// Split on both separators rather than through `Path`, because a Windows
    /// path is still a Windows path when this runs on a Unix host — which it
    /// does, in the tests and wherever a configuration file is read for a
    /// machine other than the one reading it.
    pub fn of(program: &str) -> Self {
        let basename = program
            .rsplit(['/', '\\'])
            .find(|part| !part.is_empty())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match basename.strip_suffix(".exe").unwrap_or(&basename) {
            "bash" => Self::Bash,
            "zsh" => Self::Zsh,
            "fish" => Self::Fish,
            "powershell" | "pwsh" => Self::PowerShell,
            _ => Self::Other,
        }
    }
}

/// Whether these arguments make the shell run a command and exit, in which case
/// there is no interactive session to install anything into.
pub fn runs_a_command(args: &[String]) -> bool {
    args.iter().any(|argument| {
        let argument = argument.to_ascii_lowercase();
        matches!(
            argument.as_str(),
            "-c" | "--command"
                | "/c"
                | "/k"
                | "-command"
                | "-commandwithargs"
                | "-encodedcommand"
                | "-encodedarguments"
                | "-file"
        ) || (argument.starts_with('-')
            // Short clusters only: `-ic` runs a command, `--norc` does not.
            // Testing every option that merely contains a 'c' silently denied
            // the shell integration to a profile configured as `bash --norc`
            // or `zsh --no-globalrcs`.
            && !argument.starts_with("--")
            && argument[1..].contains('c'))
    })
}

/// The line that loads the shell integration, or `None` for a shell that has
/// none. Carriage-return terminated, because it is delivered as if typed.
pub fn shell_integration_startup_command(kind: ShellKind, args: &[String]) -> Option<Vec<u8>> {
    if runs_a_command(args) {
        return None;
    }
    // Deliberately per platform rather than per shell: a Cygwin or MSYS2 bash
    // on Windows reaches Zetta through a launcher, and the `zetta` its `eval`
    // would call is not the one on that shell's `PATH`.
    #[cfg(windows)]
    let command = match kind {
        ShellKind::PowerShell => {
            r#"if (-not $global:__ZettaLifecycleTrackerInstalled -or -not $global:__ZettaLifecycleTrackingEnabled) { & $env:ZETTA_HOST_EXECUTABLE init powershell | Out-String | Invoke-Expression }"#
        }
        _ => return None,
    };
    #[cfg(not(windows))]
    let command = match kind {
        ShellKind::Bash => {
            r#"if [[ ${__ZETTA_LIFECYCLE_TRACKING_INSTALLED:-0} != 1 || ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} != 1 ]]; then eval "$(command zetta init bash)"; fi"#
        }
        ShellKind::Zsh => {
            r#"if [[ ${__ZETTA_LIFECYCLE_TRACKING_VERSION:-0} != 3 || ( -n ${ZETTA_PANE_ROUTING_ID:-${ZETTA_PANE_ID:-}} && ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} != 1 ) ]]; then eval "$(command zetta init zsh)"; fi"#
        }
        ShellKind::Fish => {
            r#"if not set -q __ZETTA_LIFECYCLE_TRACKING_INSTALLED; or test "$__ZETTA_LIFECYCLE_TRACKING_ENABLED" != 1; command zetta init fish | source; end"#
        }
        ShellKind::PowerShell | ShellKind::Other => return None,
    };
    let mut command = command.as_bytes().to_vec();
    command.push(b'\r');
    Some(command)
}

/// The concealed marker the wrapper prints once the shell is ready for its
/// payload, and the title it reports when the payload has run.
///
/// These strings are matched by whoever drives the handshake, so they live
/// beside the wrapper that emits them. Two processes drive it — the
/// application for a pane it owns, and the daemon for a pane it is adding to a
/// shared session — and a difference between their two copies would be a
/// handshake that never completes.
pub const INIT_COMMAND_MARKER_PREFIX: &str = "__zed_init_command_ready_";
pub const INIT_COMMAND_DONE_TITLE_PREFIX: &str = "zetta-init-command-done:";
pub const INIT_COMMAND_HISTORY_PREFIX: &str = "__zed_init_command_history_";
pub const INIT_COMMAND_MARKER_SUFFIX: &str = "__";

pub fn init_command_marker(marker_id: u64) -> String {
    format!("{INIT_COMMAND_MARKER_PREFIX}{marker_id}{INIT_COMMAND_MARKER_SUFFIX}")
}

pub fn init_command_done_title(marker_id: u64) -> String {
    format!("{INIT_COMMAND_DONE_TITLE_PREFIX}{marker_id}")
}

/// The line that makes a shell announce it is ready, wait for a payload, run
/// it, and announce that it has.
///
/// The marker is printed in pieces so the shell's own echo of this line cannot
/// satisfy the handshake: only the `printf` output contains it contiguously.
/// `None` for a shell with no wrapper, which is every shell that has no
/// integration to load.
pub fn init_command_wrapper(kind: ShellKind, marker_id: u64) -> Option<String> {
    let marker = format!(
        "printf '\\033[8m%s%s%s\\033[0m\\n' {INIT_COMMAND_MARKER_PREFIX} {marker_id} \
         {INIT_COMMAND_MARKER_SUFFIX}"
    );
    let done = format!("printf '\\033]2;{INIT_COMMAND_DONE_TITLE_PREFIX}%s\\033\\\\' {marker_id}");
    let history = format!("{INIT_COMMAND_HISTORY_PREFIX}{marker_id}{INIT_COMMAND_MARKER_SUFFIX}");
    match kind {
        ShellKind::Bash | ShellKind::Zsh => Some(format!(
            " if [ -n \"${{BASH_VERSION:-}}\" ]; then builtin history -d -1 2>/dev/null || :; fi; \
             stty -echo; {marker}; IFS= read -r __zed_init_command_ready_payload; \
             if [ -n \"$__zed_init_command_ready_payload\" ]; then \
             eval \"$__zed_init_command_ready_payload\"; fi; stty echo; {done}; : # {history}"
        )),
        ShellKind::Fish => Some(format!(
            " stty -echo -icanon min 1; {marker}; read --null __zed_init_command_ready_payload; \
             if test -n \"$__zed_init_command_ready_payload\"; \
             eval \"$__zed_init_command_ready_payload\"; end; stty echo icanon; {done}; # {history}"
        )),
        ShellKind::PowerShell | ShellKind::Other => None,
    }
}
