use super::*;

use std::collections::{HashMap, HashSet};

/// Absolute directories in the host's own spelling, so `is_absolute` means on
/// this host what it means to Windows for `C:\…`.
fn root(name: &str) -> PathBuf {
    std::env::temp_dir().join("zetta-program-search").join(name)
}

fn files(paths: &[PathBuf]) -> impl Fn(&Path) -> bool + use<> {
    let files = paths.iter().cloned().collect::<HashSet<_>>();
    move |path| files.contains(path)
}

fn directories(path: &[PathBuf]) -> SearchDirectories {
    SearchDirectories {
        application: Some(root("app")),
        system: vec![root("system32"), root("windows")],
        path: Some(std::env::join_paths(path).unwrap()),
    }
}

#[test]
fn a_bare_name_is_never_looked_up_in_the_current_directory() {
    // A planted `cmd.exe` in the directory the pane opens in — which is the
    // launcher's current directory — must lose to the system one, and an
    // empty or relative `PATH` entry naming that directory must not find it.
    let planted = PathBuf::from("cmd.exe");
    let relative = PathBuf::from("bin").join("cmd.exe");
    let system = root("system32").join("cmd.exe");
    let is_file = files(&[planted.clone(), relative.clone(), system.clone()]);
    let directories = directories(&[PathBuf::new(), PathBuf::from("."), PathBuf::from("bin")]);

    assert_eq!(resolve_program("cmd.exe", &directories, &is_file), Resolution::Found(system));
}

#[test]
fn relative_and_empty_path_entries_are_skipped() {
    let is_file = files(&[PathBuf::from("tool.exe"), PathBuf::from("bin").join("tool.exe")]);
    let directories = directories(&[PathBuf::new(), PathBuf::from("."), PathBuf::from("bin")]);

    assert_eq!(resolve_program("tool.exe", &directories, is_file), Resolution::Missing);
}

#[test]
fn the_search_order_is_application_then_system_then_path() {
    let application = root("app").join("pwsh.exe");
    let system = root("windows").join("pwsh.exe");
    let on_path = root("pwsh7").join("pwsh.exe");
    let directories = directories(&[root("pwsh7")]);

    let everywhere = files(&[application.clone(), system.clone(), on_path.clone()]);
    assert_eq!(
        resolve_program("pwsh.exe", &directories, everywhere),
        Resolution::Found(application)
    );
    let not_beside_us = files(&[system.clone(), on_path.clone()]);
    assert_eq!(resolve_program("pwsh.exe", &directories, not_beside_us), Resolution::Found(system));
    let only_on_path = files(std::slice::from_ref(&on_path));
    assert_eq!(resolve_program("pwsh.exe", &directories, only_on_path), Resolution::Found(on_path));
}

#[test]
fn a_name_without_an_extension_is_found_as_an_exe() {
    let system = root("system32").join("powershell.exe");
    let is_file = files(std::slice::from_ref(&system));

    assert_eq!(
        resolve_program("powershell", &directories(&[]), &is_file),
        Resolution::Found(system)
    );
    // An extension of its own is taken as written, as `CreateProcessW` does.
    assert_eq!(resolve_program("powershell.com", &directories(&[]), &is_file), Resolution::Missing);
}

#[test]
fn an_absolute_path_is_kept_and_a_relative_one_is_left_alone() {
    let absolute = root("tools").join("shell.exe");
    let is_file = files(std::slice::from_ref(&absolute));
    let directories = directories(&[]);

    assert_eq!(
        resolve_program(&absolute.to_string_lossy(), &directories, &is_file),
        Resolution::Found(absolute.clone())
    );
    // `lpApplicationName` gets no default extension, so one the command line
    // would have had added is added here instead.
    let extensionless = root("tools").join("shell");
    assert_eq!(
        resolve_program(&extensionless.to_string_lossy(), &directories, &is_file),
        Resolution::Found(absolute)
    );
    assert_eq!(resolve_program(r"bin\shell.exe", &directories, &is_file), Resolution::Relative);
    assert_eq!(resolve_program("bin/shell.exe", &directories, &is_file), Resolution::Relative);
}

#[test]
fn a_profile_path_overrides_the_inherited_one_case_insensitively() {
    let overrides = HashMap::from([("Path".to_owned(), "profile".to_owned())]);
    assert_eq!(effective_path(&overrides, Some("inherited".into())), Some("profile".into()));
    assert_eq!(effective_path(&HashMap::new(), Some("inherited".into())), Some("inherited".into()));
}

#[test]
fn the_program_token_is_split_as_create_process_splits_it() {
    assert_eq!(command_line_program(r#""C:\Program Files\x.exe" -a"#), r"C:\Program Files\x.exe");
    assert_eq!(
        command_line_program(r"C:\Windows\System32\cmd.exe /d"),
        r"C:\Windows\System32\cmd.exe"
    );
    assert_eq!(command_line_program("cmd.exe\t/d"), "cmd.exe");
    assert_eq!(command_line_program(r#""unterminated"#), "unterminated");
}

#[test]
fn a_program_is_quoted_only_when_it_would_split() {
    assert_eq!(quote_program(r"C:\Windows\System32\cmd.exe"), r"C:\Windows\System32\cmd.exe");
    assert_eq!(quote_program(r"C:\Program Files\x.exe"), r#""C:\Program Files\x.exe""#);
    assert_eq!(quote_program(""), r#""""#);
    assert_eq!(
        command_line_program(&quote_program(r"C:\Program Files\x.exe")),
        r"C:\Program Files\x.exe"
    );
}
