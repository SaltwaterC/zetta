use super::*;
use std::path::PathBuf;
use terminal::PathLikeTarget;
use util::paths::{PathStyle, home_dir};

#[test]
fn relative_file_links_are_resolved_from_the_terminal_directory() {
    let terminal_dir = std::env::temp_dir().join("zetta-link-test");
    let target = PathLikeTarget {
        maybe_path: "src/main.rs:12:4".to_owned(),
        terminal_dir: Some(terminal_dir.clone()),
        path_style: PathStyle::local(),
        spaced_readings: Vec::new(),
        host_root: None,
    };

    assert_eq!(
        resolve_local_path(&target),
        terminal_dir.join("src").join("main.rs")
    );
}

#[test]
fn local_home_paths_are_expanded_after_position_suffixes_are_parsed() {
    let target = PathLikeTarget {
        maybe_path: "~/source/zetta/src/main.rs:12:4".to_owned(),
        terminal_dir: None,
        path_style: PathStyle::local(),
        spaced_readings: Vec::new(),
        host_root: None,
    };

    assert_eq!(
        resolve_local_path(&target),
        home_dir().join("source/zetta/src/main.rs")
    );
}

#[test]
fn a_bare_local_home_path_is_expanded() {
    let target = PathLikeTarget {
        maybe_path: "~".to_owned(),
        terminal_dir: None,
        path_style: PathStyle::local(),
        spaced_readings: Vec::new(),
        host_root: None,
    };

    assert_eq!(resolve_local_path(&target), *home_dir());
}

#[test]
fn non_home_prefixed_paths_are_not_tilde_expanded() {
    for maybe_path in ["source/zetta", "~other-user/source/zetta", "project/~/file"] {
        let target = PathLikeTarget {
            maybe_path: maybe_path.to_owned(),
            terminal_dir: None,
            path_style: PathStyle::local(),
            spaced_readings: Vec::new(),
            host_root: None,
        };

        assert_eq!(resolve_local_path(&target), PathBuf::from(maybe_path));
    }
}

#[cfg(windows)]
#[test]
fn wsl_home_paths_are_left_for_the_shell() {
    let target = PathLikeTarget {
        maybe_path: "~/source/zetta".to_owned(),
        terminal_dir: None,
        path_style: PathStyle::Unix,
        spaced_readings: Vec::new(),
        host_root: None,
    };

    assert_eq!(resolve_local_path(&target), PathBuf::from("~/source/zetta"));
}

#[cfg(windows)]
#[test]
fn wsl_relative_file_links_preserve_the_linux_working_directory() {
    let target = PathLikeTarget {
        maybe_path: "ssh/sshd_config".to_owned(),
        terminal_dir: Some(PathBuf::from("/etc")),
        path_style: PathStyle::Unix,
        spaced_readings: Vec::new(),
        host_root: None,
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
            spaced_readings: Vec::new(),
            host_root: None,
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
        spaced_readings: Vec::new(),
        host_root: None,
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
        spaced_readings: Vec::new(),
        host_root: None,
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
        spaced_readings: Vec::new(),
        host_root: None,
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
        spaced_readings: Vec::new(),
        host_root: None,
    };

    assert_eq!(
        local_path_open_action(&target),
        LocalPathOpenAction::RevealFile(file)
    );
}

/// A fresh directory for one test's spaced names, removed when dropped.
struct SpacedNames(PathBuf);

impl SpacedNames {
    fn new(test: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("zetta-spaced-links-{}-{test}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }

    fn create(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, "").unwrap();
        path
    }

    fn target(&self, maybe_path: &str, readings: &[&str]) -> PathLikeTarget {
        Self::target_in(&self.0, maybe_path, readings)
    }

    fn target_in(
        directory: &std::path::Path,
        maybe_path: &str,
        readings: &[&str],
    ) -> PathLikeTarget {
        let anywhere = terminal::Range::new(terminal::Point::new(0, 0), terminal::Point::new(0, 0));
        PathLikeTarget {
            maybe_path: maybe_path.to_owned(),
            terminal_dir: Some(directory.to_path_buf()),
            path_style: PathStyle::local(),
            spaced_readings: readings
                .iter()
                .map(|reading| SpacedPathReading {
                    maybe_path: (*reading).to_owned(),
                    range: anywhere,
                })
                .collect(),
            host_root: None,
        }
    }
}

impl Drop for SpacedNames {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_clicked_fragment_opens_the_longest_spaced_reading_that_exists() {
    let names = SpacedNames::new("longest");
    names.create("Q3");
    let whole = names.create("Q3 report.xlsx");
    let target = names.target(
        "report.xlsx",
        &["ls Q3 report.xlsx", "Q3 report.xlsx", "ls Q3", "Q3"],
    );

    assert_eq!(
        existing_spaced_reading(&target).map(|reading| reading.maybe_path.as_str()),
        Some("Q3 report.xlsx")
    );
    assert_eq!(resolve_local_path(&with_existing_reading(target)), whole);
}

#[test]
fn a_fragment_whose_readings_do_not_exist_stays_as_matched() {
    let names = SpacedNames::new("missing");
    let target = names.target("report.xlsx", &["Q3 report.xlsx"]);

    assert_eq!(existing_spaced_reading(&target), None);
    let target = with_existing_reading(target);
    assert_eq!(target.maybe_path, "report.xlsx");
    assert!(target.spaced_readings.is_empty());
}

#[test]
fn a_spaced_reading_is_checked_without_its_position_suffix() {
    let names = SpacedNames::new("suffix");
    let whole = names.create("my module.rs");
    let target = names.target("module.rs:12:4", &["my module.rs:12:4"]);

    assert_eq!(resolve_local_path(&with_existing_reading(target)), whole);
}

/// A WSL link as the terminal reports it: Linux spelling, with `root` standing
/// in for `\\wsl.localhost\<distribution>`.
fn wsl_target(
    root: &std::path::Path,
    maybe_path: &str,
    terminal_dir: Option<&str>,
    readings: &[&str],
) -> PathLikeTarget {
    let mut target = SpacedNames::target_in(root, maybe_path, readings);
    target.terminal_dir = terminal_dir.map(PathBuf::from);
    target.path_style = PathStyle::Unix;
    target.host_root = Some(root.to_path_buf());
    target
}

#[test]
fn a_wsl_link_is_checked_and_revealed_under_the_distribution_root() {
    let names = SpacedNames::new("wsl");
    std::fs::create_dir_all(names.0.join("home/user")).unwrap();
    let whole = names.create("home/user/Q3 report.xlsx");
    let target = wsl_target(
        &names.0,
        "report.xlsx",
        Some("/home/user"),
        &["Q3 report.xlsx"],
    );

    let target = with_existing_reading(target);
    assert_eq!(target.maybe_path, "Q3 report.xlsx");
    assert_eq!(
        local_path_open_action(&target),
        LocalPathOpenAction::RevealFile(whole)
    );
    // The editor runs inside the distribution and keeps its spelling.
    assert_eq!(
        resolve_local_path(&target),
        PathBuf::from("/home/user/Q3 report.xlsx")
    );
}

#[test]
fn a_wsl_directory_link_opens_under_the_distribution_root() {
    let names = SpacedNames::new("wsl-directory");
    std::fs::create_dir_all(names.0.join("home/user")).unwrap();
    let target = wsl_target(&names.0, "/home/user", None, &[]);

    assert_eq!(
        local_path_open_action(&target),
        LocalPathOpenAction::OpenDirectory(names.0.join("home").join("user"))
    );
}

#[test]
fn a_wsl_link_cannot_climb_out_of_its_distribution() {
    let names = SpacedNames::new("wsl-climb");
    let target = wsl_target(&names.0, "/../../etc/./hosts", None, &[]);

    assert_eq!(
        local_path_open_action(&target),
        LocalPathOpenAction::RevealFile(names.0.join("etc").join("hosts"))
    );
}

#[test]
fn a_relative_wsl_link_with_no_reported_directory_is_left_unresolved() {
    let names = SpacedNames::new("wsl-relative");
    names.create("report.xlsx");
    let target = wsl_target(&names.0, "report.xlsx", None, &["Q3 report.xlsx"]);

    assert_eq!(existing_spaced_reading(&target), None);
    assert_eq!(
        local_path_open_action(&target),
        LocalPathOpenAction::RevealFile(PathBuf::from("report.xlsx"))
    );
}
