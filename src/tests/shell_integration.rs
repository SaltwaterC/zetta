use super::*;

use std::fmt::Write as _;

use zetta_sync::RecoverPoison as _;

#[path = "shell_integration/completion.rs"]
mod completion;

#[path = "shell_integration/tool_completion.rs"]
mod tool_completion;

#[path = "shell_integration/fish_completion.rs"]
mod fish_completion;

#[path = "shell_integration/startup_scripts.rs"]
mod startup_scripts;

#[path = "shell_integration/configuration.rs"]
mod configuration;

#[path = "shell_integration/lazy_loading.rs"]
mod lazy_loading;

// Keep the external Bash tests serialized, including tests that invoke Bash
// alongside other shells.
pub(super) fn lock_bash_tests() -> std::sync::MutexGuard<'static, ()> {
    static BASH_TEST_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    BASH_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .recover_poison()
}

fn bash_command() -> std::process::Command {
    clean_shell_command(bash_program().expect("bash availability checked before running the test"))
}

pub(super) fn bash_available() -> bool {
    bash_program().is_some()
}

/// Select a responsive Bash once for all tests. Windows' first `bash.exe` is
/// often the WSL launcher; use a native Bash so a stalled WSL startup cannot
/// hold the shared test lock indefinitely.
fn bash_program() -> Option<&'static std::path::Path> {
    static BASH: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    BASH.get_or_init(|| {
        #[cfg(windows)]
        {
            std::env::var_os("PATH")
                .into_iter()
                .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
                .map(|dir| dir.join("bash.exe"))
                .filter(|path| path.is_file() && !is_windows_bash_launcher(path))
                .find(|path| bash_responds(path))
        }
        #[cfg(not(windows))]
        {
            let path = std::path::PathBuf::from("bash");
            bash_responds(&path).then_some(path)
        }
    })
    .as_deref()
}

#[cfg(windows)]
fn is_windows_bash_launcher(path: &std::path::Path) -> bool {
    path.parent()
        .and_then(std::path::Path::file_name)
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|dir| {
            dir.eq_ignore_ascii_case("System32") || dir.eq_ignore_ascii_case("WindowsApps")
        })
}

fn bash_responds(path: &std::path::Path) -> bool {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let Ok(mut child) = clean_shell_command(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Err(_) => return false,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Runs `driver` under `command` with the shell integration script prepended,
/// and returns its stdout once the script has been asserted to run cleanly.
///
/// `command` is a parameter rather than built here because three of these tests
/// need extra environment and one needs a working directory; the rest pass
/// [`bash_completion_command`].
pub(super) fn run_bash_driver(mut command: std::process::Command, driver: &str) -> String {
    use std::io::Write as _;
    use std::process::Stdio;

    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(driver.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Bash completion script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A Bash that reads neither profile nor rc, so only the script under test is
/// in scope.
pub(super) fn bash_completion_command() -> std::process::Command {
    let mut command = bash_command();
    command.args(["--noprofile", "--norc"]);
    command
}

/// Whether the Bash under test has `compopt` to be told its candidates are
/// file names, so it quotes them as it inserts them. Bash 3.2, macOS's own,
/// has not: there the completion offers a name that holds a space already
/// quoted, or the space would end the word.
pub(super) fn bash_has_compopt() -> bool {
    static HAS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HAS.get_or_init(|| {
        run_bash_driver(bash_completion_command(), "type -t compopt || true\n").trim() == "builtin"
    })
}

/// A runtime name as the Bash under test is offered it: as it is, or with its
/// spaces escaped where Bash cannot quote it itself ([`bash_has_compopt`]).
pub(super) fn bash_offered(name: &str) -> String {
    if bash_has_compopt() {
        name.to_owned()
    } else {
        name.replace(' ', "\\ ")
    }
}

pub(super) fn clean_shell_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    // Do not let a user's shell startup environment change the exit status or
    // behavior of a generated script under test.
    command
        .env_remove("BASH_ENV")
        .env_remove("BASHOPTS")
        .env_remove("SHELLOPTS")
        .env_remove("ZETTA_PROCESS_ID")
        .env_remove("ZETTA_ATTENTION_ID")
        .env_remove("ZETTA_PANE_ID")
        .env_remove("ZETTA_PANE_ROUTING_ID")
        .env_remove("ZETTA_NO_MUX")
        .env_remove("__ZETTA_COMMAND_MARKER_NONCE");
    // Nor let it reach the terminal the tests were started from.
    #[cfg(unix)]
    crate::startup::detached_from_terminal(&mut command);
    command
}
