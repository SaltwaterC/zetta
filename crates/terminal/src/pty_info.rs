use gpui::{Context, Task};
use parking_lot::{MappedRwLockReadGuard, Mutex, RwLock, RwLockReadGuard};
#[cfg(unix)]
use std::sync::atomic::{AtomicI32, Ordering};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use sysinfo::{Pid, Process, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::{Event, Terminal};

pub struct ProcessIdGetter {
    /// The pty master descriptor, borrowed rather than owned: the pty event
    /// loop closes it when it stops. [`ProcessIdGetter::close`] marks it unusable
    /// at that point, because the number is free to be reassigned to an
    /// unrelated file — including another pane's pty master — and asking a
    /// recycled descriptor for its foreground process group would answer with
    /// somebody else's job.
    #[cfg(unix)]
    handle: AtomicI32,
    #[cfg(windows)]
    pid: Option<u32>,
    fallback_pid: u32,
}

/// The value [`ProcessIdGetter::handle`] holds once the pty has been closed.
/// `tcgetpgrp` rejects it, but it is never passed to one: the guard in
/// [`ProcessIdGetter::pid`] comes first.
#[cfg(unix)]
const CLOSED_PTY_HANDLE: i32 = -1;

impl ProcessIdGetter {
    #[cfg(unix)]
    pub(crate) fn new(handle: i32, fallback_pid: u32) -> ProcessIdGetter {
        ProcessIdGetter {
            handle: AtomicI32::new(handle),
            fallback_pid,
        }
    }

    #[cfg(windows)]
    pub(crate) fn new(pid: Option<u32>, fallback_pid: u32) -> ProcessIdGetter {
        ProcessIdGetter { pid, fallback_pid }
    }

    pub fn fallback_pid(&self) -> Pid {
        Pid::from_u32(self.fallback_pid)
    }
}

#[cfg(unix)]
impl ProcessIdGetter {
    fn pid(&self) -> Option<Pid> {
        let handle = self.handle.load(Ordering::Relaxed);
        if handle != CLOSED_PTY_HANDLE {
            // Negative pid means error.
            // Zero pid means no foreground process group is set on the PTY yet.
            // Avoid killing the current process by returning a zero pid.
            let pid = unsafe { libc::tcgetpgrp(handle) };
            if pid > 0 {
                return Some(Pid::from_u32(pid as u32));
            }
        }

        if self.fallback_pid > 0 {
            return Some(Pid::from_u32(self.fallback_pid));
        }

        None
    }

    /// Stops using the pty master descriptor. Called when the event loop that
    /// owns it is released; afterwards only [`Self::fallback_pid`] is reported.
    fn close(&self) {
        self.handle.store(CLOSED_PTY_HANDLE, Ordering::Relaxed);
    }
}

#[cfg(not(unix))]
impl ProcessIdGetter {
    fn close(&self) {}
}

#[cfg(windows)]
impl ProcessIdGetter {
    fn pid(&self) -> Option<Pid> {
        self.pid
            .or_else(|| (self.fallback_pid > 0).then_some(self.fallback_pid))
            .map(Pid::from_u32)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProcessInfo {
    pub(crate) name: String,
    pub(crate) cwd: PathBuf,
    pub(crate) argv: Vec<String>,
}

/// Process groups captured while the PTY master is still open. A foreground
/// job may have a different process group from the shell, so closing only the
/// shell group can leave a child process orphaned.
#[derive(Clone, Copy)]
pub(crate) struct TerminalProcessIds {
    #[cfg(unix)]
    foreground: Option<Pid>,
    #[cfg(unix)]
    child: Pid,
}

#[cfg(unix)]
impl TerminalProcessIds {
    fn process_group_ids(self) -> impl Iterator<Item = i32> {
        std::iter::once(self.child)
            .chain(
                self.foreground
                    .filter(|foreground| *foreground != self.child),
            )
            .map(|pid| pid.as_u32() as i32)
            .filter(|pid| *pid > 0)
    }

    pub(crate) fn terminate(&self) -> bool {
        self.signal(libc::SIGTERM)
    }

    pub(crate) fn kill(&self) -> bool {
        self.signal(libc::SIGKILL)
    }

    fn signal(&self, signal: i32) -> bool {
        let mut signalled = false;
        for process_group_id in self.process_group_ids() {
            signalled |= unsafe { libc::killpg(process_group_id, signal) } == 0;
        }
        signalled
    }
}

#[cfg(not(unix))]
impl TerminalProcessIds {
    pub(crate) fn terminate(&self) -> bool {
        false
    }

    pub(crate) fn kill(&self) -> bool {
        false
    }
}

/// Fetches Zed-relevant Pseudo-Terminal (PTY) process information
pub(crate) struct PtyProcessInfo {
    system: RwLock<System>,
    refresh_kind: ProcessRefreshKind,
    pid_getter: ProcessIdGetter,
    last_foreground_pid: Mutex<Option<Pid>>,
    pub(crate) current: RwLock<Option<ProcessInfo>>,
    task: Mutex<Option<Task<()>>>,
    refresh_pending: Mutex<bool>,
    last_refresh: Mutex<Option<Instant>>,
    // ConPTY only exposes the PID of the process it spawned (the shell); unlike
    // Unix's `tcgetpgrp`, there is no OS primitive to ask which process currently
    // owns the terminal. This is a dedicated `System` for periodically walking the
    // live process tree to approximate an answer (see `resolve_foreground_pid`).
    #[cfg(windows)]
    descendant_tree: RwLock<System>,
}

const PROCESS_INFO_REFRESH_INTERVAL: Duration = Duration::from_millis(500);

#[cfg(target_os = "macos")]
fn is_macos_login_shell(
    login_name: Option<&str>,
    login_pid: Pid,
    foreground_pid: Pid,
    foreground_parent: Option<Pid>,
) -> bool {
    login_name == Some("login")
        && foreground_pid != login_pid
        && foreground_parent == Some(login_pid)
}

fn process_refresh_due(last_refresh: Option<Instant>, now: Instant) -> bool {
    process_refresh_delay(last_refresh, now).is_zero()
}

fn process_refresh_delay(last_refresh: Option<Instant>, now: Instant) -> Duration {
    last_refresh.map_or(Duration::ZERO, |last| {
        PROCESS_INFO_REFRESH_INTERVAL.saturating_sub(now.saturating_duration_since(last))
    })
}

impl PtyProcessInfo {
    pub(crate) fn new(pid_getter: ProcessIdGetter) -> PtyProcessInfo {
        // Task enumeration is on by default and would retain a `Process` entry
        // per thread, each pinning an open `/proc/<pid>/task/<tid>/stat` handle
        // on Linux (#58651).
        let process_refresh_kind = ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_cwd(UpdateKind::Always)
            .with_exe(UpdateKind::Always)
            .without_tasks();
        // `System::new_with_specifics` with a process refresh kind would
        // snapshot every process on the machine into this terminal's `System`,
        // retaining one open procfs handle per process for the lifetime of the
        // terminal (#58651). Refresh only the spawned child so that
        // `kill_child_process` works before the first foreground refresh.
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid_getter.fallback_pid()]),
            true,
            process_refresh_kind,
        );

        PtyProcessInfo {
            system: RwLock::new(system),
            refresh_kind: process_refresh_kind,
            pid_getter,
            last_foreground_pid: Mutex::new(None),
            current: RwLock::new(None),
            task: Mutex::new(None),
            refresh_pending: Mutex::new(false),
            last_refresh: Mutex::new(None),
            #[cfg(windows)]
            descendant_tree: RwLock::new(System::new()),
        }
    }

    pub(crate) fn pid_getter(&self) -> &ProcessIdGetter {
        &self.pid_getter
    }

    pub(crate) fn capture_process_ids(&self) -> TerminalProcessIds {
        TerminalProcessIds {
            #[cfg(unix)]
            foreground: self.pid_getter.pid(),
            #[cfg(unix)]
            child: self.pid_getter.fallback_pid(),
        }
    }

    /// Releases the pty master descriptor this borrows for foreground-process
    /// lookups. Must be called by whoever releases the pty event loop, before
    /// the descriptor can be reassigned. See [`ProcessIdGetter`].
    pub(crate) fn close_pty_handle(&self) {
        self.pid_getter.close();
    }

    #[cfg(all(test, unix))]
    pub(crate) fn pty_handle_is_open(&self) -> bool {
        self.pid_getter.handle.load(Ordering::Relaxed) != CLOSED_PTY_HANDLE
    }

    #[cfg(unix)]
    fn resolve_foreground_pid(&self) -> Option<Pid> {
        self.pid_getter.pid()
    }

    /// Best-effort stand-in for Unix's `tcgetpgrp`: walk the live process tree
    /// from the shell down to its most recently started childless descendant,
    /// on the assumption that's the interactively running command. Refreshing
    /// with `ProcessesToUpdate::All` and `remove_dead_processes = true` keeps
    /// `descendant_tree` bounded to currently-alive processes rather than
    /// accumulating stale entries (see the accumulation concern in `refresh`
    /// below / #58651).
    #[cfg(windows)]
    fn resolve_foreground_pid(&self) -> Option<Pid> {
        const MAX_DEPTH: usize = 32;
        let shell_pid = self.pid_getter.pid()?;
        let mut system = self.descendant_tree.write();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        let mut current = shell_pid;
        for _ in 0..MAX_DEPTH {
            let Some(child) = system
                .processes()
                .values()
                .filter(|process| process.parent() == Some(current))
                .max_by_key(|process| process.start_time())
            else {
                break;
            };
            current = child.pid();
        }
        Some(current)
    }

    /// Returns whether the process currently owning the terminal is the shell
    /// process that was created for the PTY. Unknown process state is treated
    /// as non-shell so callers can choose the safe fallback.
    #[allow(dead_code)]
    pub(crate) fn foreground_process_is_shell(&self) -> bool {
        self.foreground_process_is_shell_context() == Some(true)
    }

    /// Returns the shell ownership decision when the platform can make one.
    /// `None` is different from `Some(false)`: the former means that process
    /// metadata was unavailable or the observed foreground process no longer
    /// exists, while the latter means a live foreground command was observed.
    pub(crate) fn foreground_process_is_shell_context(&self) -> Option<bool> {
        let foreground =
            (*self.last_foreground_pid.lock()).or_else(|| self.resolve_foreground_pid())?;

        let shell = self.pid_getter.fallback_pid();
        if shell.as_u32() == 0 {
            return None;
        }
        if foreground == shell {
            return Some(true);
        }

        // The cached foreground pid can be up to one refresh interval stale,
        // and at exit time it often names a process that has already
        // terminated. A dead foreground is not a running command: report
        // unknown so the exit is not mislabeled as a foreground-command
        // failure (a normal `exit`/Ctrl-D close).
        #[cfg(target_os = "macos")]
        return self.foreground_process_is_macos_login_shell(foreground, shell);

        #[cfg(not(target_os = "macos"))]
        self.foreground_process_alive(foreground).then_some(false)
    }

    /// Whether the given process currently exists on the system. Refreshing
    /// just this pid evicts a stale entry for a process that has already
    /// exited (sysinfo keeps entries absent from the refreshed set — see the
    /// accumulation note in `refresh`), so this both answers the question and
    /// keeps the process map bounded. macOS covers this with the two-pid
    /// refresh in `foreground_process_is_macos_login_shell`.
    #[cfg(not(target_os = "macos"))]
    fn foreground_process_alive(&self, pid: Pid) -> bool {
        let mut system = self.system.write();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing(),
        );
        system.process(pid).is_some()
    }

    #[cfg(target_os = "macos")]
    fn foreground_process_is_macos_login_shell(&self, foreground: Pid, shell: Pid) -> Option<bool> {
        // The default macOS shell is started as `/usr/bin/login ... /bin/zsh`.
        // `login` can remain as the PTY child while the shell it starts owns
        // the foreground process group, so the two PIDs are not always equal.
        let mut system = self.system.write();
        let pids = [foreground, shell];
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&pids),
            true,
            ProcessRefreshKind::nothing(),
        );

        let Some(login) = system.process(shell) else {
            return None;
        };
        let Some(foreground_process) = system.process(foreground) else {
            return None;
        };

        Some(is_macos_login_shell(
            login.name().to_str(),
            shell,
            foreground,
            foreground_process.parent(),
        ))
    }

    fn refresh(&self) -> Option<MappedRwLockReadGuard<'_, Process>> {
        let pid = self.resolve_foreground_pid()?;
        let fallback_pid = self.pid_getter.fallback_pid();
        let mut system = self.system.write();
        // sysinfo never evicts processes that are absent from the refreshed pid
        // set, so entries for former foreground processes (each pinning an open
        // `/proc/<pid>/stat` handle on Linux) would otherwise accumulate for as
        // long as this terminal lives (#58651). Rebuild the `System` whenever
        // the foreground process changes to keep the map bounded.
        if self.last_foreground_pid.lock().replace(pid) != Some(pid) {
            *system = System::new();
        }
        let pids = [pid, fallback_pid];
        let pids = if pid == fallback_pid {
            &pids[..1]
        } else {
            &pids[..]
        };
        system.refresh_processes_specifics(ProcessesToUpdate::Some(pids), true, self.refresh_kind);
        *self.last_refresh.lock() = Some(Instant::now());
        drop(system);
        RwLockReadGuard::try_map(self.system.read(), |system| system.process(pid)).ok()
    }

    fn get_child(&self) -> Option<MappedRwLockReadGuard<'_, Process>> {
        let pid = self.pid_getter.fallback_pid();
        RwLockReadGuard::try_map(self.system.read(), |system| system.process(pid)).ok()
    }

    #[cfg(unix)]
    pub(crate) fn kill_current_process(&self) -> bool {
        let Some(pid) = self.pid_getter.pid() else {
            return false;
        };
        unsafe { libc::killpg(pid.as_u32() as i32, libc::SIGKILL) == 0 }
    }

    #[cfg(not(unix))]
    pub(crate) fn kill_current_process(&self) -> bool {
        self.refresh().is_some_and(|process| process.kill())
    }

    pub(crate) fn kill_child_process(&self) -> bool {
        self.get_child().is_some_and(|process| process.kill())
    }

    fn load(&self) -> Option<ProcessInfo> {
        let process = self.refresh()?;
        let cwd = process.cwd().map_or(PathBuf::new(), |p| p.to_owned());

        let info = ProcessInfo {
            name: process.name().to_str()?.to_owned(),
            cwd,
            argv: process
                .cmd()
                .iter()
                .filter_map(|s| s.to_str().map(ToOwned::to_owned))
                .collect(),
        };
        *self.current.write() = Some(info.clone());
        Some(info)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn load_for_test(&self) -> Option<ProcessInfo> {
        self.load()
    }

    #[cfg(unix)]
    fn cheap_foreground_pid_changed(&self) -> bool {
        self.pid_getter.pid() != *self.last_foreground_pid.lock()
    }

    /// Unlike `tcgetpgrp` on Unix, there is no cheap way to detect a foreground-
    /// process change on Windows: `resolve_foreground_pid` needs a fresh,
    /// system-wide process snapshot. Rely solely on the periodic refresh timer
    /// (`process_refresh_due`) instead of an eager check.
    #[cfg(windows)]
    fn cheap_foreground_pid_changed(&self) -> bool {
        false
    }

    /// Updates the cached process info, emitting a [`Event::TitleChanged`] event if the Zed-relevant info has changed
    pub(crate) fn emit_title_changed_if_changed(self: &Arc<Self>, cx: &mut Context<'_, Terminal>) {
        if self.task.lock().is_some() {
            *self.refresh_pending.lock() = true;
            return;
        }
        let foreground_pid_changed = self.cheap_foreground_pid_changed();
        let now = Instant::now();
        let last_refresh = *self.last_refresh.lock();
        let refresh_delay = if foreground_pid_changed || process_refresh_due(last_refresh, now) {
            Duration::ZERO
        } else {
            process_refresh_delay(last_refresh, now)
        };
        let this = self.clone();
        let executor = cx.background_executor().clone();
        let change_task = cx.background_executor().spawn(async move {
            if !refresh_delay.is_zero() {
                executor.timer(refresh_delay).await;
            }
            // Wakeups received before this scan are covered by it. Only a
            // wakeup racing with the scan itself needs another pass.
            *this.refresh_pending.lock() = false;
            let previous = this.current.read().clone();
            let current = this.load();
            let has_changed = match (previous.as_ref(), current.as_ref()) {
                (None, None) => false,
                (Some(prev), Some(now)) => {
                    prev.cwd != now.cwd || prev.name != now.name || prev.argv != now.argv
                }
                _ => true,
            };
            let changed_cwd = match (previous.as_ref(), current.as_ref()) {
                (Some(prev), Some(now)) if prev.cwd != now.cwd => Some(now.cwd.clone()),
                (None, Some(now)) => Some(now.cwd.clone()),
                _ => None,
            };
            if has_changed {
                *this.current.write() = current;
            }
            (has_changed, changed_cwd)
        });
        let this = Arc::downgrade(self);
        *self.task.lock() = Some(cx.spawn(async move |term, cx| {
            let (has_changed, changed_cwd) = change_task.await;
            if has_changed {
                term.update(cx, |terminal, cx| {
                    if let Some(cwd) = changed_cwd {
                        terminal.record_cwd_change(cwd);
                    }
                    cx.emit(Event::TitleChanged);
                })
                .ok();
            }
            if let Some(this) = this.upgrade() {
                this.task.lock().take();
                if std::mem::take(&mut *this.refresh_pending.lock()) {
                    term.update(cx, Terminal::refresh_foreground_process).ok();
                }
            }
        }));
    }

    pub(crate) fn pid(&self) -> Option<Pid> {
        self.pid_getter.pid()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn foreground_process_identity_matches_the_pty_shell_pid() {
        let shell = PtyProcessInfo::new(ProcessIdGetter::new(-1, std::process::id()));
        assert!(shell.foreground_process_is_shell());

        let unknown = PtyProcessInfo::new(ProcessIdGetter::new(-1, 0));
        assert!(!unknown.foreground_process_is_shell());
    }

    #[cfg(windows)]
    #[test]
    fn windows_process_id_getter_keeps_the_full_pid_width() {
        let getter = ProcessIdGetter::new(Some(u32::MAX), 1);
        assert_eq!(getter.pid(), Some(Pid::from_u32(u32::MAX)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_login_wrapper_is_only_an_interactive_shell_when_its_child_owns_the_tty() {
        let login_pid = Pid::from_u32(10);
        let shell_pid = Pid::from_u32(11);
        let command_pid = Pid::from_u32(12);

        assert!(is_macos_login_shell(
            Some("login"),
            login_pid,
            shell_pid,
            Some(login_pid)
        ));
        assert!(!is_macos_login_shell(
            Some("login"),
            login_pid,
            command_pid,
            Some(shell_pid)
        ));
        assert!(!is_macos_login_shell(
            Some("zsh"),
            login_pid,
            shell_pid,
            Some(login_pid)
        ));
    }

    #[test]
    fn process_refreshes_are_throttled() {
        let now = Instant::now();
        assert!(process_refresh_due(None, now));
        assert!(!process_refresh_due(
            Some(now),
            now + Duration::from_millis(499)
        ));
        assert!(process_refresh_due(
            Some(now),
            now + PROCESS_INFO_REFRESH_INTERVAL
        ));
    }

    #[test]
    fn throttled_process_refreshes_are_deferred() {
        let now = Instant::now();

        assert_eq!(process_refresh_delay(None, now), Duration::ZERO);
        assert_eq!(
            process_refresh_delay(Some(now), now + Duration::from_millis(200)),
            Duration::from_millis(300)
        );
        assert_eq!(
            process_refresh_delay(Some(now), now + PROCESS_INFO_REFRESH_INTERVAL),
            Duration::ZERO
        );
    }

    /// Regression test for <https://github.com/zed-industries/zed/issues/58651>:
    /// on Linux, sysinfo keeps an open `/proc/<pid>/stat` handle for every
    /// `Process` entry retained in a `System`, and never evicts entries that are
    /// absent from the refreshed pid set. The per-terminal `System` must
    /// therefore not snapshot every process on the machine, nor accumulate an
    /// entry per foreground process that has ever run in this terminal.
    #[cfg(unix)]
    #[test]
    #[allow(
        clippy::disallowed_methods,
        reason = "the test needs real short-lived child processes and may block"
    )]
    fn process_map_stays_bounded() {
        let mut info = PtyProcessInfo::new(ProcessIdGetter::new(-1, std::process::id()));
        assert!(
            info.get_child().is_some(),
            "the spawned child must be inspectable for kill_child_process \
             before the first foreground refresh"
        );
        assert!(info.load_for_test().is_some());
        let initial_len = info.system.read().processes().len();
        assert!(
            initial_len <= 2,
            "creating a terminal retained {initial_len} process entries"
        );

        for _ in 0..3 {
            let mut child = std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("failed to spawn child process");
            info.pid_getter = ProcessIdGetter::new(-1, child.id());
            assert!(info.load_for_test().is_some());
            child.kill().expect("failed to kill child process");
            child.wait().expect("failed to wait for child process");
        }

        let churned_len = info.system.read().processes().len();
        assert!(
            churned_len <= 2,
            "foreground process churn retained {churned_len} process entries"
        );
    }

    #[cfg(unix)]
    #[test]
    #[allow(
        clippy::disallowed_methods,
        reason = "the test needs real short-lived child processes and may block"
    )]
    fn a_dead_foreground_process_is_unknown_not_a_running_command() {
        let info = PtyProcessInfo::new(ProcessIdGetter::new(-1, std::process::id()));
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("failed to spawn child process");

        *info.last_foreground_pid.lock() = Some(Pid::from_u32(child.id()));
        assert_eq!(
            info.foreground_process_is_shell_context(),
            Some(false),
            "a live foreground process is a running command"
        );

        child.kill().expect("failed to kill child process");
        child.wait().expect("failed to wait for child process");
        assert_eq!(
            info.foreground_process_is_shell_context(),
            None,
            "a foreground pid that no longer exists is unknown, not a failure"
        );
    }
}
