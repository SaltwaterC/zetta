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

/// The protocol names a remote attach may ask for are checked here rather than
/// in whichever window picks the request up, so a typo fails where it was
/// typed.
#[test]
fn attach_takes_a_protocol_and_refuses_an_unknown_one() {
    assert_eq!(parse_remote_protocol("ZOSH").unwrap(), "zosh");
    assert_eq!(parse_remote_protocol(" ssh ").unwrap(), "ssh");

    let error = parse_remote_protocol("mosh").unwrap_err().to_string();
    assert!(error.contains("ssh"), "{error}");
    assert!(error.contains("zosh"), "{error}");

    let error = run(&args(&["attach", "--protocol"]))
        .unwrap_err()
        .to_string();
    assert!(error.contains("--protocol requires"), "{error}");
}

/// `-k` takes no separate value, so `zmux attach -k host 42` still names a
/// target; an interval is given with `=`, exactly as `zosh -k` spells it.
#[test]
fn keep_alive_defaults_without_a_value_and_bounds_one_that_is_given() {
    assert_eq!(parse_keep_alive_interval("250").unwrap(), 250);
    assert!(parse_keep_alive_interval("5").is_err());
    assert!(parse_keep_alive_interval("99999").is_err());
    assert!(parse_keep_alive_interval("soon").is_err());

    // It holds a Zosh link open, so asking for one without asking for Zosh is
    // a mistake rather than a setting that quietly does nothing.
    let error = run(&args(&["attach", "host", "42", "--keep-alive"]))
        .unwrap_err()
        .to_string();
    assert!(error.contains("--protocol zosh"), "{error}");
}

/// Both belong to `attach`; naming one anywhere else is a mistake worth
/// reporting rather than an option that is silently ignored.
#[test]
fn the_remote_protocol_options_belong_to_attach() {
    for arguments in [
        args(&["list", "--protocol", "zosh"]),
        args(&["reconnect", "42", "--keep-alive=250"]),
    ] {
        let error = run(&arguments).unwrap_err().to_string();
        assert!(error.contains("only valid with attach"), "{error}");
    }
}

/// The help has to name both, because a user who cannot see an option cannot
/// choose it.
#[test]
fn the_protocol_options_are_documented() {
    let help = usage(false);
    assert!(help.contains("-P, --protocol NAME"), "{help}");
    assert!(help.contains("-k, --keep-alive"), "{help}");
    assert!(help.contains("relay-pane SESSION_ID PANE_ID"), "{help}");
}

/// The viewer a pane is relayed to is a `relay-pane` matter only. Naming it
/// anywhere else would read as a way to act as another client, which it is not.
#[test]
fn the_relayed_viewer_belongs_to_relay_pane() {
    for arguments in [
        args(&["list", "--viewer-stdin"]),
        args(&["create", "-w"]),
        args(&["attach", "dev.example", "7", "--viewer-stdin"]),
    ] {
        let error = run(&arguments).unwrap_err().to_string();
        assert!(
            error.contains("--viewer-stdin is only valid with relay-pane"),
            "{error}"
        );
    }
    assert!(
        usage(false).contains("-w, --viewer-stdin"),
        "{}",
        usage(false)
    );
}
