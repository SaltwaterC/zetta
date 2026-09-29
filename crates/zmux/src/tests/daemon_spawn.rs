use super::*;

#[test]
fn plain_words_are_joined_unquoted() {
    assert_eq!(
        command_line(&[r"C:\Zetta\zmux.exe", "--daemon", "--retention", "memory"]),
        r"C:\Zetta\zmux.exe --daemon --retention memory"
    );
}

#[test]
fn words_with_spaces_quotes_or_nothing_are_quoted_so_they_split_back() {
    assert_eq!(
        command_line(&[r"C:\Program Files\Zetta\zmux.exe", ""]),
        r#""C:\Program Files\Zetta\zmux.exe" """#
    );
    // A backslash only escapes when a quote follows it.
    assert_eq!(command_line(&[r#"say "hi""#]), r#""say \"hi\"""#);
    assert_eq!(
        command_line(&[r"C:\dir with space\"]),
        r#""C:\dir with space\\""#
    );
    assert_eq!(command_line(&[r#"a\"b c"#]), r#""a\\\"b c""#);
}

#[cfg(unix)]
#[test]
fn a_detached_start_returns_without_waiting_for_the_daemon() {
    let started = std::time::Instant::now();
    spawn_detached(
        Path::new("/bin/sh"),
        &[OsString::from("-c"), OsString::from("sleep 5")],
    )
    .unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}
