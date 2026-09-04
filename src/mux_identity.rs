//! The configured age identity, read for the multiplexer command line.
//!
//! `zmux` is deliberately free of Zetta's configuration format, but a user who
//! set `sessions.persistence.identity` once should not have to repeat it as
//! `--identity` on every `zmux resume`, `zmux reconnect`, or protected remote
//! session command. When no identity is configured, the conventional
//! `~/.ssh/id_ed25519` is used if it exists. The two entry points
//! that ship with Zetta — the `zmux` binary and `zetta mux` — therefore read it
//! here and pass it in as a [`zmux::ClientDefaults`].
//!
//! Deliberately not `Config::load`: `src/bin/zmux.rs` is its own binary with no
//! library to share, and parsing the whole configuration — profiles, keymap,
//! themes — to reach one path would make every short-lived multiplexer command
//! pay for it. One field is read instead, and
//! `the_command_line_identity_reader_agrees_with_the_configuration_parser` in
//! `src/tests/config.rs` keeps that from drifting from the real parser. The
//! guard lives there rather than here because this file is also compiled into a
//! binary that has no `Config` to compare against.

use std::path::PathBuf;

/// The identity files a multiplexer command should try, from configuration or
/// the conventional `~/.ssh/id_ed25519` fallback.
///
/// An explicitly configured identity takes precedence, even when its path is
/// missing, so the resulting error names the configured path. The fallback is
/// used only when no identity is configured and the file exists.
pub(crate) fn configured_identity_paths(config_path: Option<PathBuf>) -> Vec<PathBuf> {
    configured_identity_paths_with_fallback(config_path, zmux::persistence::default_identity_path())
}

pub(crate) fn configured_identity_paths_with_fallback(
    config_path: Option<PathBuf>,
    fallback: Option<PathBuf>,
) -> Vec<PathBuf> {
    let fallback = || fallback.clone().into_iter().collect::<Vec<_>>();
    let path =
        config_path.unwrap_or_else(|| zmux::paths::platform_config_dir().join("config.json"));
    let Ok(contents) = std::fs::read_to_string(path) else {
        return fallback();
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return fallback();
    };
    let identity = root
        .get("sessions")
        .and_then(|sessions| sessions.get("persistence"))
        .and_then(|persistence| persistence.get("identity"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|identity| !identity.is_empty());
    identity
        .map(expand_home)
        .map_or_else(fallback, |identity| vec![identity])
}

/// Expands a leading `~/`, which is how an identity is conventionally written in
/// configuration and how the application's own parser resolves it.
fn expand_home(path: &str) -> PathBuf {
    let Some(relative) = path.strip_prefix("~/") else {
        return PathBuf::from(path);
    };
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join(relative)
}

/// Whether a `zmux` command can need an identity, and so is worth reading
/// configuration for. `resume` decrypts a disk record for fresh-shell restore;
/// `reconnect` and remote commands may have to open a session sealed to the
/// user's key. Everything else — `list` above all — is a short-lived process
/// that should not pay for a file it will not use.
pub(crate) fn command_uses_an_identity(arguments: &[std::ffi::OsString]) -> bool {
    arguments.iter().any(|argument| {
        matches!(
            argument.to_str(),
            Some("resume" | "reconnect" | "attach" | "kill" | "share" | "unshare" | "forget")
        )
    })
}

// Inline rather than in `src/tests/`, unlike the rest of `src/`: this file is
// compiled into two binaries from two directories — normally, and through
// `#[path = "../mux_identity.rs"]` in `src/bin/zmux.rs` — and a sidecar path is
// resolved relative to the including file, so no single one resolves for both.
#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(persistence: &str) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        std::fs::write(
            &path,
            format!(r#"{{"sessions": {{"persistence": {persistence}}}}}"#),
        )
        .unwrap();
        (directory, path)
    }

    #[test]
    fn reads_the_configured_identity() {
        let (_directory, path) = config_with(r#"{"identity": "/keys/zetta.txt"}"#);
        assert_eq!(
            configured_identity_paths(Some(path)),
            vec![PathBuf::from("/keys/zetta.txt")]
        );
    }

    #[test]
    fn expands_a_leading_home_shorthand() {
        let (_directory, path) = config_with(r#"{"identity": "~/keys/zetta.txt"}"#);
        let resolved = configured_identity_paths(Some(path));
        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].ends_with("keys/zetta.txt"));
        assert!(!resolved[0].starts_with("~"));
    }

    #[test]
    fn absent_unset_and_blank_identities_all_read_as_none() {
        for persistence in [r#"{}"#, r#"{"identity": null}"#, r#"{"identity": "  "}"#] {
            let (_directory, path) = config_with(persistence);
            assert!(
                configured_identity_paths_with_fallback(Some(path), None).is_empty(),
                "{persistence} should resolve to no identity"
            );
        }
    }

    #[test]
    fn uses_the_default_ssh_identity_when_no_identity_is_configured() {
        let (_directory, path) = config_with(r#"{}"#);
        let default = PathBuf::from("/home/test/.ssh/id_ed25519");
        assert_eq!(
            configured_identity_paths_with_fallback(Some(path), Some(default.clone())),
            vec![default]
        );
    }

    #[test]
    fn an_explicit_identity_takes_precedence_over_the_default_ssh_identity() {
        let (_directory, path) = config_with(r#"{"identity":"~/configured.txt"}"#);
        let default = PathBuf::from("/home/test/.ssh/id_ed25519");
        let resolved = configured_identity_paths_with_fallback(Some(path), Some(default));
        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].ends_with("configured.txt"));
        assert!(!resolved[0].ends_with("id_ed25519"));
    }

    /// A command line that cannot read configuration still uses an explicitly
    /// supplied fallback, rather than refusing to run at all.
    #[test]
    fn an_unreadable_or_malformed_configuration_yields_no_identity() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("absent.json");
        assert!(configured_identity_paths_with_fallback(Some(missing), None).is_empty());

        let malformed = directory.path().join("malformed.json");
        std::fs::write(&malformed, "{ this is not json").unwrap();
        assert!(configured_identity_paths_with_fallback(Some(malformed), None).is_empty());
    }

    #[test]
    fn only_the_commands_that_can_need_an_identity_ask_for_one() {
        let arguments = |command: &str| vec![std::ffi::OsString::from(command)];
        assert!(command_uses_an_identity(&arguments("resume")));
        assert!(command_uses_an_identity(&arguments("reconnect")));
        for command in ["attach", "kill", "share", "unshare", "forget"] {
            assert!(
                command_uses_an_identity(&arguments(command)),
                "{command} should load configuration"
            );
        }
        for command in ["list", "stop"] {
            assert!(
                !command_uses_an_identity(&arguments(command)),
                "{command} should not load configuration"
            );
        }
        assert!(!command_uses_an_identity(&[]));
    }
}
