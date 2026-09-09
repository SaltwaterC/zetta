//! Restore-command validation shared by local sessions.

pub(crate) const MAX_RESTORE_COMMAND_BYTES: usize = 64 * 1024;

pub(crate) fn valid_restore_command(command: &str) -> Option<String> {
    (!command.is_empty()
        && command.len() <= MAX_RESTORE_COMMAND_BYTES
        && !command.chars().any(char::is_control))
    .then(|| command.to_owned())
}
