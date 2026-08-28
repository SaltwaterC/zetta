use super::*;

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

#[test]
fn format_help_table_aligns_multiline_rows_without_trailing_whitespace() {
    let help = format_help_table([
        ("short", "first description\ncontinued description"),
        ("long label", "second description"),
    ]);
    let lines = help.lines().collect::<Vec<_>>();
    let description_column = 2 + "long label".chars().count() + 2;

    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].find("first description"), Some(description_column));
    assert_eq!(
        lines[1].find("continued description"),
        Some(description_column)
    );
    assert_eq!(
        lines[2].find("second description"),
        Some(description_column)
    );
    assert!(lines.iter().all(|line| *line == line.trim_end()));
}

#[test]
fn kill_requires_a_session_id() {
    let error = run(&args(&["kill"])).unwrap_err().to_string();
    assert!(error.contains("requires a session ID"), "{error}");

    let error = run(&args(&["kill", "not-a-number"]))
        .unwrap_err()
        .to_string();
    assert!(error.contains("positive whole number"), "{error}");
}

#[test]
fn reconnect_requires_a_session_id_and_is_documented_separately_from_share() {
    let error = run(&args(&["reconnect"])).unwrap_err().to_string();
    assert!(error.contains("requires a session ID"), "{error}");
    assert!(usage(false).contains("reconnect SESSION_ID"));
    assert!(usage(false).contains("it does not open it"));
}

#[test]
fn no_mux_usage_only_documents_commands_that_do_not_need_a_daemon() {
    assert!(usage(true).contains("list"));
    assert!(usage(true).contains("reconnect SESSION_ID"));
    for daemon_command in ["stop", "share", "unshare", "kill", "forget"] {
        assert!(
            !usage(true).contains(daemon_command),
            "no-mux usage must omit {daemon_command}"
        );
    }
    assert!(!usage(true).contains("--force"));
    assert!(!usage(true).contains("--upgrade"));
    assert!(!NO_MUX_SESSION_ID_HELP.contains("share"));
    assert!(usage(false).contains("--upgrade"));
}

#[test]
fn retention_needs_a_mode_and_rejects_unknown_ones() {
    let error = run(&args(&["--retention"])).unwrap_err().to_string();
    assert!(error.contains("requires a mode"), "{error}");

    let error = run(&args(&["--retention", "everything"]))
        .unwrap_err()
        .to_string();
    assert!(error.contains("unknown retention"), "{error}");
}

#[test]
fn unknown_arguments_are_refused() {
    assert!(run(&args(&["--nonsense"])).is_err());
    assert!(run(&args(&["frobnicate"])).is_err());
}

#[test]
fn stop_is_a_command_and_force_is_its_flag() {
    // `stop` takes no session: it is the multiplexer itself being stopped, and
    // a stray id there would otherwise be read as `kill`'s.
    let error = run(&args(&["stop", "7"])).unwrap_err().to_string();
    assert!(error.contains("unknown mux argument"), "{error}");

    assert!(usage(false).contains("stop"), "stop must be documented");
    assert!(
        usage(false).contains("--force"),
        "--force must be documented"
    );
    // Both spellings, as every other flag here has.
    for flag in ["--force", "-f"] {
        let error = run(&args(&[flag, "frobnicate"])).unwrap_err().to_string();
        assert!(
            error.contains("frobnicate"),
            "{flag} must parse rather than be refused itself: {error}"
        );
    }
}

#[test]
fn version_takes_the_same_short_form_as_zetta() {
    // `zetta -v` prints its version, so `zmux -v` has to as well: one of them
    // taking `-V` instead is a difference nobody can remember which way round.
    for flag in ["--version", "-v", "-V"] {
        run(&args(&[flag])).unwrap_or_else(|error| panic!("{flag}: {error}"));
    }
    assert!(
        usage(false).contains("-v, --version"),
        "-v must be documented"
    );
    // And the protocol with it: which build this is says nothing about whether it
    // can talk to the multiplexer that is running, which is the question.
    assert!(
        usage(false).contains("the protocol it speaks"),
        "the protocol must be part of what --version promises"
    );
}

#[test]
fn upgrade_has_a_short_form() {
    // Parsed, not run: the trailing nonsense fails the parse loop before the
    // upgrade would be attempted, which is what keeps this test from replacing
    // whatever multiplexer the machine happens to be running.
    for flag in ["--upgrade", "-u"] {
        let error = run(&args(&[flag, "frobnicate"])).unwrap_err().to_string();
        assert!(
            error.contains("frobnicate"),
            "{flag} must parse rather than be refused itself: {error}"
        );
    }
    assert!(
        usage(false).contains("-u, --upgrade"),
        "-u must be documented"
    );
}

#[test]
fn an_ambiguous_bare_session_id_requires_the_full_identifier() {
    let session = |process_id, runner_id| protocol::BackgroundSessionCatalog {
        version: protocol::CATALOG_VERSION,
        process_id,
        runner_id,
        sessions: vec![protocol::BackgroundSessionSummary {
            id: 1,
            title: "shell".to_owned(),
            authentication_required: false,
            active_pane: 1,
            layout: protocol::BackgroundPaneLayout::Pane { pane_id: 1 },
            panes: Vec::new(),
            held: false,
            scoped_to: None,
            key_envelope: None,
        }],
    };

    ensure_unambiguous_session_id(&[session(123, 7)], 1).unwrap();
    let error = ensure_unambiguous_session_id(&[session(123, 7), session(456, 8)], 1)
        .unwrap_err()
        .to_string();
    assert!(error.contains("ambiguous"), "{error}");
    assert!(error.contains("PROCESS:RUNNER:SESSION"), "{error}");
}
