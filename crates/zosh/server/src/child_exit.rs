//! Wakes the session loop when the session's child process exits.
//!
//! The loop used to learn of an exit by calling `try_wait` on every pass,
//! which is a system call per pass and a reason to keep passing. PTY end of
//! file is not a substitute: a background job can hold the terminal open after
//! the shell has gone, and on Windows a ConPTY's output pipe stays open after
//! its client exits until the pseudoconsole itself is closed.
//!
//! A watcher thread therefore blocks until the process has exited and then
//! wakes the loop, which calls `try_wait` itself. The watcher only *observes*
//! the exit — `waitid(WNOWAIT)` on Unix, a duplicated process handle on
//! Windows — so the loop still owns the child, reaps it, and kills it the way
//! `portable_pty` does, a hangup first and a kill after a grace period, which
//! a killer handed to another thread could not do.
//!
//! Where no watcher can be started the loop falls back to polling the child on
//! a slow timer rather than on every pass.

use portable_pty::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, Thread};
use std::time::Duration;

/// How often the loop polls a child it could not attach a watcher to.
pub const FALLBACK_POLL: Duration = Duration::from_millis(250);

/// Set once the watched child has exited.
pub struct ChildExitWatch {
    exited: Arc<AtomicBool>,
}

impl ChildExitWatch {
    /// Starts watching `child`, waking `consumer` when it exits. `None` when
    /// this platform or this child cannot be watched.
    pub fn start(child: &dyn Child, consumer: Thread) -> Option<Self> {
        let wait = platform::exit_waiter(child)?;
        let exited = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&exited);
        thread::Builder::new()
            .name("zosh-child-exit".to_owned())
            .spawn(move || {
                wait();
                flag.store(true, Ordering::Release);
                consumer.unpark();
            })
            .ok()?;
        Some(Self { exited })
    }

    /// Whether the child has exited since the watch started.
    pub fn exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }
}

#[cfg(unix)]
mod platform {
    use portable_pty::Child;

    /// Blocks until the child exits, leaving it unreaped for `try_wait`.
    pub fn exit_waiter(child: &dyn Child) -> Option<impl FnOnce() + Send + 'static> {
        let pid = libc::id_t::try_from(child.process_id()?).ok()?;
        Some(move || {
            loop {
                // SAFETY: `info` is a plain C struct that waitid only writes.
                let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                // SAFETY: waitid is given a valid pointer to `info`. WNOWAIT
                // leaves the child waitable, so the loop's own `try_wait`
                // still reaps it and still owns its exit status.
                let result = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        pid,
                        &raw mut info,
                        libc::WEXITED | libc::WNOWAIT,
                    )
                };
                // EINTR is retried. Anything else — ECHILD once the loop has
                // reaped the child itself — means there is nothing left to
                // wait for, and waking the loop for it is harmless.
                if result == 0
                    || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
                {
                    return;
                }
            }
        })
    }
}

#[cfg(windows)]
mod platform {
    use portable_pty::Child;
    use windows::Win32::Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
    use windows::Win32::System::Threading::{GetCurrentProcess, INFINITE, WaitForSingleObject};

    /// A process handle owned by the watcher thread. A `HANDLE` is a raw
    /// pointer and so not `Send`, but a kernel handle is usable from any
    /// thread of the process that owns it.
    struct OwnedProcess(HANDLE);

    // SAFETY: see the type's documentation; only the watcher thread uses it.
    unsafe impl Send for OwnedProcess {}

    impl Drop for OwnedProcess {
        fn drop(&mut self) {
            // SAFETY: the handle was duplicated for this value alone.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    /// Blocks until the child exits. The handle is duplicated so the wait
    /// does not depend on how long the loop keeps the child's own.
    pub fn exit_waiter(child: &dyn Child) -> Option<impl FnOnce() + Send + 'static> {
        let source = HANDLE(child.as_raw_handle()?.cast());
        let mut duplicate = HANDLE::default();
        // SAFETY: both process handles are pseudo-handles for this process,
        // `source` is the child's live handle, and `duplicate` is written
        // only on success.
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                source,
                GetCurrentProcess(),
                &mut duplicate,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
        }
        .ok()?;
        let process = OwnedProcess(duplicate);
        Some(move || {
            // SAFETY: the handle stays open until `process` drops below.
            unsafe { WaitForSingleObject(process.0, INFINITE) };
            drop(process);
        })
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use portable_pty::Child;

    pub fn exit_waiter(_child: &dyn Child) -> Option<fn()> {
        None
    }
}

#[cfg(test)]
#[path = "tests/child_exit.rs"]
mod tests;
