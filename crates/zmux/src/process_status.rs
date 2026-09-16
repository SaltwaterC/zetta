//! Direct process-liveness checks used by attachment ownership and reconnect.
//!
//! A missed live process is destructive for the multiplexer: it makes the
//! daemon reclaim an attached PTY and race its real reader. Unix therefore
//! asks the kernel about the one PID rather than deriving the answer from a
//! fallible whole-process enumeration.

#[cfg(unix)]
pub(crate) fn is_running(process_id: u32) -> bool {
    let Ok(process_id) = libc::pid_t::try_from(process_id) else {
        return false;
    };
    if process_id == 0 {
        return false;
    }

    // SAFETY: signal zero delivers no signal; it only asks whether this PID
    // exists and whether the caller may signal it.
    if unsafe { libc::kill(process_id, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
pub(crate) fn is_running(process_id: u32) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let process_id = Pid::from_u32(process_id);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[process_id]), true);
    system.process(process_id).is_some()
}

#[cfg(test)]
#[path = "tests/process_status.rs"]
mod tests;
