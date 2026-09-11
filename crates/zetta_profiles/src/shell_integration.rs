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
        ) || (argument.starts_with('-') && argument[1..].contains('c'))
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
