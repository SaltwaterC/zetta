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
    if executable.file_name().and_then(|name| name.to_str()) != Some("zmux") {
        assert_eq!(arguments.first().map(String::as_str), Some("mux"));
    }
}
