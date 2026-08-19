use super::*;

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
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

    assert!(USAGE.contains("stop"), "stop must be documented");
    assert!(USAGE.contains("--force"), "--force must be documented");
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
    assert!(USAGE.contains("-v, --version"), "-v must be documented");
    // And the protocol with it: which build this is says nothing about whether it
    // can talk to the multiplexer that is running, which is the question.
    assert!(
        USAGE.contains("the protocol it speaks"),
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
    assert!(USAGE.contains("-u, --upgrade"), "-u must be documented");
}
