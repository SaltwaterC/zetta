use super::*;

#[test]
fn a_typed_secret_has_to_be_confirmed() {
    let error = confirmed(Zeroizing::new("hunter2".to_owned()), "hunter3")
        .expect_err("a mistyped secret must be refused")
        .to_string();
    assert!(error.contains("do not match"), "{error}");

    let secret = confirmed(Zeroizing::new("hunter2".to_owned()), "hunter2")
        .expect("a confirmed secret is the session's");
    assert_eq!(secret.expose(), "hunter2");
}

#[test]
fn a_typed_secret_keeps_whatever_is_not_the_line_ending() {
    for (typed, expected) in [
        ("hunter2\n", "hunter2"),
        ("hunter2\r\n", "hunter2"),
        // Only the ending: a secret may legitimately end in a space.
        ("hunter2 \n", "hunter2 "),
        ("", ""),
    ] {
        let mut line = typed.to_owned();
        strip_line_ending(&mut line);
        assert_eq!(line, expected, "{typed:?}");
    }
}
