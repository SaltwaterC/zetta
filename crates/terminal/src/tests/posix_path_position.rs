use super::*;

#[test]
fn a_separator_the_host_rewrote_is_read_back_from_the_text() {
    // What `parse_str` returns on Windows, where `with_file_name` joins with `\`.
    assert_eq!(
        written_path(
            "/home/testuser/source/zetta/main.rs:12:4",
            Path::new(r"/home/testuser/source/zetta\main.rs"),
        ),
        Some("/home/testuser/source/zetta/main.rs")
    );
}

#[test]
fn a_backslash_the_text_holds_is_kept() {
    assert_eq!(
        written_path(r"/srv/odd\name.rs:3", Path::new(r"/srv/odd\name.rs")),
        Some(r"/srv/odd\name.rs")
    );
}

#[test]
fn a_parsed_path_that_is_not_a_prefix_of_the_text_is_left_alone() {
    assert_eq!(written_path("/a/b.rs:1", Path::new("/a/c.rs")), None);
    assert_eq!(written_path("/a.rs", Path::new("/a.rs/longer")), None);
}

#[test]
fn the_position_is_still_parsed() {
    let parsed = parse_posix_path_with_position("/cygdrive/c/Users/testuser/main.rs:12:4");
    assert_eq!(
        parsed.path,
        PathBuf::from("/cygdrive/c/Users/testuser/main.rs")
    );
    assert_eq!(parsed.row, Some(12));
    assert_eq!(parsed.column, Some(4));

    let parsed = parse_posix_path_with_position("/home/testuser/notes");
    assert_eq!(parsed.path, PathBuf::from("/home/testuser/notes"));
    assert_eq!(parsed.row, None);
}
