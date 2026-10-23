use super::*;

#[test]
fn the_current_process_is_running() {
    assert!(is_running(std::process::id()));
}

#[cfg(unix)]
#[test]
fn values_that_are_not_process_ids_are_not_running() {
    assert!(!is_running(0));
    assert!(!is_running(u32::MAX));
}

#[cfg(unix)]
#[test]
fn a_reaped_process_is_not_running() {
    let mut child = std::process::Command::new("/usr/bin/true")
        .spawn()
        .expect("spawning a short-lived process");
    let process_id = child.id();
    child.wait().expect("reaping the short-lived process");
    assert!(!is_running(process_id));
}

#[test]
fn a_parent_is_read_past_a_command_name_with_spaces_and_parentheses() {
    assert_eq!(
        parent_from_stat("4242 (zsh) S 177514 4242 4242 0"),
        Some(177514)
    );
    assert_eq!(
        parent_from_stat("4242 (a (weird) name) S 9 4242 4242 0"),
        Some(9)
    );
    assert_eq!(parent_from_stat("garbage"), None);
}

#[cfg(target_os = "linux")]
#[test]
fn this_process_is_described_with_its_command_and_parent() {
    let described = describe(std::process::id());
    assert!(
        described.starts_with(&format!("process {} (", std::process::id())),
        "{described}"
    );
    assert!(described.contains(", child of "), "{described}");
}

#[cfg(all(unix, not(target_os = "linux")))]
#[test]
fn this_process_description_has_its_pid_without_requiring_procfs() {
    // Command and parent details are best effort on Unix hosts without /proc.
    let described = describe(std::process::id());
    assert!(
        described.starts_with(&format!("process {}", std::process::id())),
        "{described}"
    );
}

#[test]
fn a_start_time_is_read_past_a_command_name_with_spaces_and_parentheses() {
    let fields = "S 9 4242 4242 0 -1 4194560 1 2 3 4 5 6 7 8 20 0 1 0 987654 23 24";
    assert_eq!(
        start_time_from_stat(&format!("4242 (zsh) {fields}")),
        Some(987654)
    );
    assert_eq!(
        start_time_from_stat(&format!("4242 (a (weird) 1 2) name) {fields}")),
        Some(987654)
    );
    assert_eq!(start_time_from_stat("4242 (zsh) S 9"), None);
}

#[cfg(any(target_os = "linux", target_vendor = "apple", windows))]
#[test]
fn this_process_is_recognised_by_its_start_time() {
    let this = ProcessIdentity::of(std::process::id());
    assert!(this.started.is_some(), "no start time for this process");
    assert!(this.is(std::process::id()));
    assert!(this.is_running());

    // A process that was given the same PID later is somebody else.
    let later = ProcessIdentity {
        started: this.started.map(|started| started + 1),
        ..this
    };
    assert!(!later.is(std::process::id()));
    assert!(!later.is_running());
}

#[cfg(unix)]
#[test]
fn a_reaped_process_is_no_longer_recognised() {
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("5")
        .spawn()
        .expect("spawning a short-lived process");
    let identity = ProcessIdentity::of(child.id());
    assert!(identity.is_running());
    child.kill().unwrap();
    child.wait().expect("reaping the short-lived process");
    assert!(!identity.is_running());
    assert!(!identity.is(child.id()));
}
