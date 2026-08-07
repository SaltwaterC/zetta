use super::*;
use gpui::Modifiers;
use std::path::PathBuf;
use terminal::{PathLikeTarget, is_hyperlink_modifier};
use util::paths::PathStyle;

#[test]
fn trimmed_paste_removes_only_outer_whitespace() {
    assert_eq!(
        trim_paste_text(" \t\r\n first line \n second line \r\n\t "),
        "first line \n second line"
    );
}

#[test]
fn search_navigation_wraps_in_both_directions() {
    assert_eq!(navigated_match_index(Some(2), 3, false), Some(0));
    assert_eq!(navigated_match_index(Some(0), 3, true), Some(2));
    assert_eq!(navigated_match_index(None, 0, false), None);
}

#[test]
fn search_caret_respects_utf8_boundaries() {
    let text = "aé中";
    assert_eq!(next_char_boundary(text, 1), 3);
    assert_eq!(previous_char_boundary(text, 3), 1);
}

#[test]
fn superseded_search_requests_are_rejected() {
    assert!(search_request_is_current(3, "cargo", 3, Some("cargo")));
    assert!(!search_request_is_current(3, "cargo", 4, Some("cargo")));
    assert!(!search_request_is_current(3, "cargo", 3, Some("rust")));
}

#[test]
fn disabled_input_events_are_not_allocated() {
    let builds = std::cell::Cell::new(0);
    let event = enabled_input_event(false, || {
        builds.set(builds.get() + 1);
        super::TerminalViewEvent::Input(super::TerminalInput::Text("ignored".into()))
    });
    assert!(event.is_none());
    assert_eq!(builds.get(), 0);
}

#[test]
fn relative_file_links_are_resolved_from_the_terminal_directory() {
    let terminal_dir = std::env::temp_dir().join("zetta-link-test");
    let target = PathLikeTarget {
        maybe_path: "src/main.rs:12:4".to_owned(),
        terminal_dir: Some(terminal_dir.clone()),
        path_style: PathStyle::local(),
    };

    assert_eq!(
        resolve_local_path(&target),
        terminal_dir.join("src").join("main.rs")
    );
}

#[cfg(windows)]
#[test]
fn wsl_relative_file_links_preserve_the_linux_working_directory() {
    let target = PathLikeTarget {
        maybe_path: "ssh/sshd_config".to_owned(),
        terminal_dir: Some(PathBuf::from("/etc")),
        path_style: PathStyle::Unix,
    };

    assert_eq!(
        resolve_local_path(&target),
        PathBuf::from("/etc/ssh/sshd_config")
    );
}

#[cfg(windows)]
#[test]
fn wsl_absolute_paths_do_not_require_a_reported_directory() {
    for path in [
        "/",
        "/etc/hosts",
        "/usr/local/bin/zsh",
        "/opt/service/config.toml",
        "/var/log/messages",
        "/mnt/c/Users/saltw/Desktop/notes.txt",
    ] {
        let target = PathLikeTarget {
            maybe_path: path.to_owned(),
            terminal_dir: None,
            path_style: PathStyle::Unix,
        };
        assert_eq!(resolve_local_path(&target), PathBuf::from(path));
    }
}

#[cfg(windows)]
#[test]
fn wsl_relative_paths_without_a_reported_directory_are_left_for_the_shell() {
    let target = PathLikeTarget {
        maybe_path: "../etc/hosts".to_owned(),
        terminal_dir: None,
        path_style: PathStyle::Unix,
    };

    assert_eq!(resolve_local_path(&target), PathBuf::from("../etc/hosts"));
}

#[test]
fn absolute_file_links_do_not_use_the_terminal_directory() {
    let absolute_path = std::env::temp_dir().join("zetta-absolute-link.rs");
    let target = PathLikeTarget {
        maybe_path: absolute_path.to_string_lossy().into_owned(),
        terminal_dir: Some(PathBuf::from("ignored")),
        path_style: PathStyle::local(),
    };

    assert_eq!(resolve_local_path(&target), absolute_path);
}

#[test]
fn mixed_windows_path_separators_are_normalized_for_the_shell() {
    assert_eq!(
        normalize_windows_path(PathBuf::from(
            r"C:\Users\saltw\source\repos\zetta/README.md"
        )),
        PathBuf::from(r"C:\Users\saltw\source\repos\zetta\README.md")
    );
}

#[test]
fn windows_current_directory_components_are_removed_for_the_shell() {
    assert_eq!(
        normalize_windows_path(PathBuf::from(
            r"C:\Users\saltw\source\repos\zetta/.\README.md"
        )),
        PathBuf::from(r"C:\Users\saltw\source\repos\zetta\README.md")
    );
    assert_eq!(
        normalize_windows_path(PathBuf::from(r".\.\README.md")),
        PathBuf::from("README.md")
    );
    assert_eq!(
        normalize_windows_path(PathBuf::from(r"\\.\COM1")),
        PathBuf::from(r"\\.\COM1")
    );
}

#[test]
fn directory_links_open_the_directory() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = PathLikeTarget {
        maybe_path: directory.to_string_lossy().into_owned(),
        terminal_dir: None,
        path_style: PathStyle::local(),
    };

    assert_eq!(
        local_path_open_action(&target),
        LocalPathOpenAction::OpenDirectory(directory)
    );
}

#[test]
fn file_links_are_revealed_in_their_parent_directory() {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/standalone.rs");
    let target = PathLikeTarget {
        maybe_path: file.to_string_lossy().into_owned(),
        terminal_dir: None,
        path_style: PathStyle::local(),
    };

    assert_eq!(
        local_path_open_action(&target),
        LocalPathOpenAction::RevealFile(file)
    );
}

#[test]
fn control_is_a_hyperlink_modifier_on_every_platform() {
    assert!(is_hyperlink_modifier(&Modifiers {
        control: true,
        ..Default::default()
    }));
}
