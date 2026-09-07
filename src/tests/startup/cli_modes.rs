use super::*;

#[test]
fn shell_integration_setup_message_explains_how_to_enable_a_new_configuration() {
    let message = shell_integration_configuration_message(&ShellIntegrationConfiguration::Written(
        PathBuf::from("/home/user/.bashrc"),
    ));
    assert!(message.contains("/home/user/.bashrc"));
    assert!(message.contains("Start a new shell"));
}

#[test]
fn shell_integration_setup_message_reports_an_unchanged_configuration() {
    let message = shell_integration_configuration_message(
        &ShellIntegrationConfiguration::AlreadyPresent(PathBuf::from("/home/user/.zshrc")),
    );
    assert!(message.contains("/home/user/.zshrc"));
    assert!(message.contains("no changes made"));
}

// Regression guard: `zetta pane wait -- COMMAND` used to exec argv[0]
// directly, so a shell alias or function (such as the `zvi` shortcut this
// crate's own shell integration defines) could never be found.
//
// `is_terminal_foreground: false` here is deliberate, not incidental: this
// test binary is one of hundreds of concurrently-running test threads
// sharing a single real controlling terminal when `cargo test` itself runs
// interactively, and letting a spawned shell legitimately take over that
// terminal's foreground (the `true` path) is only safe for a single
// sequential command — not for a thread among many all touching the same
// tty. Alias/function resolution is identical either way, so there is no
// reason for this test to risk it.
#[test]
fn wait_command_process_resolves_a_shell_function_the_wrapped_command_names() {
    let temporary = tempfile::tempdir().unwrap();
    std::fs::write(
        temporary.path().join(".bashrc"),
        "zetta_test_pane_wait_probe() { printf 'probe-ran:%s\\n' \"$1\"; }\n",
    )
    .unwrap();

    let output = wait_command_process(
        &Shell::Program("bash".to_owned()),
        &["zetta_test_pane_wait_probe".to_owned(), "hello".to_owned()],
        false,
    )
    .env("HOME", temporary.path())
    .output()
    .unwrap();

    assert!(
        output.status.success(),
        "wrapped shell function failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "probe-ran:hello\n");
}

// See the comment on `wait_command_process_resolves_a_shell_function_the_wrapped_command_names`
// for why `is_terminal_foreground: false` is used here too.
#[test]
fn wait_command_process_passes_metacharacters_through_unmangled() {
    let temporary = tempfile::tempdir().unwrap();

    let output = wait_command_process(
        &Shell::Program("bash".to_owned()),
        &[
            "printf".to_owned(),
            "%s\\n".to_owned(),
            "$HOME".to_owned(),
            "a b".to_owned(),
            "it's".to_owned(),
            "*.rs".to_owned(),
            String::new(),
        ],
        false,
    )
    .env("HOME", temporary.path())
    .output()
    .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "$HOME\na b\nit's\n*.rs\n\n"
    );
}

// Regression guard: routing the wrapped command through an interactive shell
// (needed to resolve aliases/functions) used to also leave the shell's own
// job control enabled whenever `is_terminal_foreground` is false. That
// job-control startup must not be allowed to run in that case: it
// unconditionally tries to acquire the controlling terminal, which can steal
// it from whatever legitimately holds it and freeze that job with `SIGTTIN`.
// `wait_command_process` prevents the attempt entirely by detaching the
// spawned shell into its own new session before it execs; a shell with no
// controlling terminal at all silently disables job control instead of
// fighting for one. This is verified by checking that the spawned process
// becomes its own session leader (Linux-only: reads its `/proc` `stat`).
//
// `is_terminal_foreground` is passed explicitly here (rather than computed
// live) so this test's outcome doesn't depend on whether it happens to run
// under a real controlling terminal that this process is the foreground of —
// which caused this exact test to fail differently depending on that when it
// computed the flag itself.
#[cfg(target_os = "linux")]
#[test]
fn wait_command_process_detaches_into_its_own_session_when_not_the_terminal_foreground() {
    let mut child = wait_command_process(
        &Shell::Program("bash".to_owned()),
        &["sleep".to_owned(), "2".to_owned()],
        false,
    )
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .spawn()
    .unwrap();

    let pid = child.id();
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    // Fields are `pid (comm) state ppid pgrp session ...`; `comm` can itself
    // contain spaces or parens, so skip past it by its closing paren instead
    // of splitting naively on whitespace from the start.
    let after_comm = stat
        .rsplit_once(')')
        .expect("/proc/<pid>/stat must have a (comm) field")
        .1;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    let session_id: u32 = fields[3].parse().expect("session field must be numeric");

    child.kill().ok();
    child.wait().ok();

    assert_eq!(
        session_id, pid,
        "the spawned shell must become its own session leader (no controlling terminal at \
         all) when this process isn't already the terminal's foreground occupant"
    );
}
