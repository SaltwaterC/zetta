//! Direct process-liveness checks used by attachment ownership and reconnect,
//! and the identity a session's owner is recognised by afterwards.
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

/// A process the daemon has to recognise again later: its PID, and when the
/// process holding that PID started.
///
/// A PID on its own names whichever process holds it *now*. That is enough for
/// a peer the kernel just vouched for, but not for one recorded earlier and
/// compared against later: once the recorded process exits, a same-user
/// process that cycles through PIDs until it is given that one would pass for
/// it. The start time is what it cannot reproduce, so it is compared afresh on
/// every check rather than once when the identity is recorded.
///
/// `started` is `None` where the platform cannot tell, and also where the
/// process had already gone when it was recorded. The first leaves the
/// comparison to the PID, which is all that platform ever had; the second
/// matches nothing that is running, because a live process does have a start
/// time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProcessIdentity {
    pub(crate) pid: u32,
    pub(crate) started: Option<u64>,
}

impl ProcessIdentity {
    /// Whichever process holds `pid` right now.
    pub(crate) fn of(pid: u32) -> Self {
        Self {
            pid,
            started: start_time(pid),
        }
    }

    /// Whether `pid`, as the process holding it now, is this one.
    pub(crate) fn is(self, pid: u32) -> bool {
        self.pid == pid && start_time(pid) == self.started
    }

    /// Whether this process, rather than another that has since been given its
    /// PID, is still running.
    pub(crate) fn is_running(self) -> bool {
        is_running(self.pid) && start_time(self.pid) == self.started
    }
}

/// When the process holding `pid` started, in units only comparable with each
/// other: clock ticks since boot on Linux, microseconds since the epoch on
/// macOS and FreeBSD, and 100 ns intervals since 1601 on Windows.
#[cfg(target_os = "linux")]
pub(crate) fn start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    start_time_from_stat(&stat)
}

/// The start time from a `/proc/PID/stat` line: field 22, counted from the
/// last `)` for the same reason as [`parent_from_stat`].
#[cfg(any(target_os = "linux", test))]
pub(crate) fn start_time_from_stat(stat: &str) -> Option<u64> {
    let (_, fields) = stat.rsplit_once(')')?;
    fields.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(target_vendor = "apple")]
pub(crate) fn start_time(pid: u32) -> Option<u64> {
    let pid = libc::c_int::try_from(pid).ok().filter(|pid| *pid > 0)?;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: the buffer and its size describe the same `proc_bsdinfo`, which
    // is what `PROC_PIDTBSDINFO` fills.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    // SAFETY: proc_pidinfo filled the whole structure, and it was zeroed before.
    let info = unsafe { info.assume_init() };
    info.pbi_start_tvsec
        .checked_mul(1_000_000)?
        .checked_add(info.pbi_start_tvusec)
}

#[cfg(target_os = "freebsd")]
pub(crate) fn start_time(pid: u32) -> Option<u64> {
    let pid = libc::c_int::try_from(pid).ok().filter(|pid| *pid > 0)?;
    let name = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    let mut info = std::mem::MaybeUninit::<libc::kinfo_proc>::zeroed();
    let mut size = std::mem::size_of::<libc::kinfo_proc>();
    // SAFETY: `name` is a valid four-element MIB, and the buffer and its size
    // describe the same `kinfo_proc`; nothing is written back.
    let result = unsafe {
        libc::sysctl(
            name.as_ptr(),
            name.len() as libc::c_uint,
            info.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null(),
            0,
        )
    };
    // A PID nobody holds answers with success and no data.
    if result != 0 || size != std::mem::size_of::<libc::kinfo_proc>() {
        return None;
    }
    // SAFETY: sysctl filled the whole structure, and it was zeroed before.
    let started = unsafe { info.assume_init() }.ki_start;
    u64::try_from(started.tv_sec)
        .ok()?
        .checked_mul(1_000_000)?
        .checked_add(u64::try_from(started.tv_usec).ok()?)
}

#[cfg(windows)]
pub(crate) fn start_time(pid: u32) -> Option<u64> {
    use windows::Win32::{
        Foundation::{CloseHandle, FILETIME},
        System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };

    // SAFETY: the access right is valid for `OpenProcess`, and the handle is
    // closed below on every path.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let [mut created, mut exited, mut kernel, mut user] = [FILETIME::default(); 4];
    // SAFETY: `process` is open with the access `GetProcessTimes` needs, and
    // every destination is a writable `FILETIME`.
    let times =
        unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) };
    // SAFETY: `process` came from `OpenProcess` and is closed exactly once.
    unsafe {
        let _ = CloseHandle(process);
    }
    times.ok()?;
    Some(u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime))
}

#[cfg(not(any(
    target_os = "linux",
    target_vendor = "apple",
    target_os = "freebsd",
    windows
)))]
pub(crate) fn start_time(_pid: u32) -> Option<u64> {
    None
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
