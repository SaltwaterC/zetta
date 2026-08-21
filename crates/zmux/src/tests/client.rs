use super::*;

#[test]
fn the_multiplexer_is_resolved_beside_this_executable_not_from_the_path() {
    // Resolving through PATH would let an unrelated `zmux` earlier in it be
    // handed a session's terminals.
    let (executable, arguments) = multiplexer_command().unwrap();

    assert!(
        executable.is_absolute(),
        "{} must not depend on PATH lookup",
        executable.display()
    );
    assert!(arguments.contains(&"--daemon".to_owned()));
    // Running as the test binary, there is no `zmux` beside it, so the
    // fallback routes through this executable's own subcommand.
    let name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let is_zmux = name == "zmux" || (cfg!(windows) && name.eq_ignore_ascii_case("zmux.exe"));
    if !is_zmux {
        assert_eq!(arguments.first().map(String::as_str), Some("mux"));
    }
}

#[test]
fn only_an_unknown_configure_variant_triggers_the_upgrade_fallback() {
    for message in [
        "unknown variant `configure`, expected `spawn`",
        "unknown variant 'configure', expected 'spawn'",
        "unknown variant \"configure\", expected \"spawn\"",
    ] {
        assert!(
            is_unsupported_configure(&anyhow::anyhow!(message)),
            "{message}"
        );
    }

    for message in [
        "unknown variant `spawn`, expected `configure`",
        "the daemon rejected the configure request",
        "unknown field `configure`",
    ] {
        assert!(
            !is_unsupported_configure(&anyhow::anyhow!(message)),
            "{message}"
        );
    }
}

#[test]
fn a_configured_zetta_daemon_starts_without_a_retention_argument() {
    let mut arguments = vec!["--daemon".to_owned()];
    append_startup_retention_arguments(&mut arguments, None);
    assert_eq!(arguments, ["--daemon"]);
}

#[test]
fn an_independent_daemon_can_still_receive_a_retention_bootstrap() {
    let mut arguments = vec!["--daemon".to_owned()];
    append_startup_retention_arguments(&mut arguments, Some(Retention::Memory { bytes: 4096 }));
    assert_eq!(
        arguments,
        [
            "--daemon",
            "--retention",
            "memory",
            "--retention-bytes",
            "4096"
        ]
    );
}

#[test]
fn an_exit_report_waits_for_a_late_shared_reporter() {
    let reporters = ExitReporters::default();
    let (sender, receiver) = async_channel::unbounded();

    reporters.report(42, Some(1792), false);
    reporters.register_shared(42, sender);

    assert_eq!(
        receiver.recv_blocking().unwrap(),
        PaneExitReport {
            raw_status: Some(1792),
            input_sent: false,
            disconnected: false,
        }
    );
}
