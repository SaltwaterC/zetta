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

/// A process as a log line can name it: its PID, its command line, and its
/// parent's, which is usually what says *why* it is running. Best effort —
/// a process that has already gone, or a platform without `/proc`, is just
/// its PID.
#[cfg(unix)]
pub(crate) fn describe(process_id: u32) -> String {
    let command = |process_id: u32| {
        std::fs::read(format!("/proc/{process_id}/cmdline"))
            .ok()
            .map(|bytes| {
                String::from_utf8_lossy(&bytes)
                    .split('\0')
                    .filter(|argument| !argument.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|command| !command.is_empty())
    };
    let parent = std::fs::read_to_string(format!("/proc/{process_id}/stat"))
        .ok()
        .and_then(|stat| parent_from_stat(&stat));
    let mut described = format!("process {process_id}");
    if let Some(command) = command(process_id) {
        described.push_str(&format!(" ({command})"));
    }
    if let Some(parent) = parent {
        described.push_str(&format!(", child of {parent}"));
        if let Some(command) = command(parent) {
            described.push_str(&format!(" ({command})"));
        }
    }
    described
}

/// The parent PID from a `/proc/PID/stat` line. The command name in the
/// second field is parenthesized and may itself contain spaces or `)`, so
/// the fields are counted from the last `)`.
#[cfg(any(unix, test))]
pub(crate) fn parent_from_stat(stat: &str) -> Option<u32> {
    let (_, fields) = stat.rsplit_once(')')?;
    fields.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(windows)]
pub(crate) fn describe(process_id: u32) -> String {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let pid = Pid::from_u32(process_id);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    let Some(process) = system.process(pid) else {
        return format!("process {process_id}");
    };
    let command = process
        .cmd()
        .iter()
        .map(|argument| argument.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    match process.parent() {
        Some(parent) => format!("process {process_id} ({command}), child of {parent}"),
        None => format!("process {process_id} ({command})"),
    }
}

#[cfg(test)]
#[path = "tests/process_status.rs"]
mod tests;
