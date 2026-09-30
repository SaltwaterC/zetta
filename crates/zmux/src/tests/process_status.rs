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
