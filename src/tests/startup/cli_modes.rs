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
