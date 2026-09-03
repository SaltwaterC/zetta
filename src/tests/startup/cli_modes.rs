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
