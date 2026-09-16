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
