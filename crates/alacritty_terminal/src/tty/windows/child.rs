use std::ffi::c_void;
use std::io::Error;
use std::num::NonZeroU32;
use std::os::windows::process::ExitStatusExt;
use std::process::ExitStatus;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use polling::os::iocp::{CompletionPacket, PollerIocpExt};
use polling::{Event, Poller};

#[cfg(test)]
use windows_sys::Win32::Foundation::GetHandleInformation;
use windows_sys::Win32::Foundation::{
    BOOLEAN, CloseHandle, FALSE, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessId, INFINITE, RegisterWaitForSingleObject, UnregisterWaitEx,
    WT_EXECUTEINWAITTHREAD, WT_EXECUTEONLYONCE,
};

use crate::tty::ChildEvent;

struct Interest {
    poller: Arc<Poller>,
    event: Event,
}

struct ChildExitSender {
    sender: mpsc::Sender<ChildEvent>,
    interest: Arc<Mutex<Option<Interest>>>,
    child_handle: AtomicPtr<c_void>,
    callback_started: Arc<std::sync::atomic::AtomicBool>,
    event_pending: Arc<std::sync::atomic::AtomicBool>,
}

/// WinAPI callback to run when child process exits.
extern "system" fn child_exit_callback(ctx: *mut c_void, timed_out: BOOLEAN) {
    let event_tx: Box<_> = unsafe { Box::from_raw(ctx as *mut ChildExitSender) };
    event_tx.callback_started.store(true, Ordering::Release);
    if timed_out != 0 {
        return;
    }

    let mut exit_code = 0_u32;
    let child_handle = event_tx.child_handle.load(Ordering::Relaxed) as HANDLE;
    let status = unsafe { GetExitCodeProcess(child_handle, &mut exit_code) };
    let event = if status == FALSE || exit_code == STILL_ACTIVE as u32 {
        ChildEvent::ExitStatusUnavailable
    } else {
        ChildEvent::Exited(ExitStatus::from_raw(exit_code))
    };
    event_tx.event_pending.store(true, Ordering::Release);
    event_tx.sender.send(event).ok();

    let interest = event_tx.interest.lock().unwrap();
    if let Some(interest) = interest.as_ref() {
        interest.poller.post(CompletionPacket::new(interest.event)).ok();
    }
}

/// Reports the exit of a child watched by another process.
///
/// Posting to the poller is what makes the event loop notice: it is waiting on
/// the poller, not on the channel, so a send alone would sit unread until some
/// other event happened to wake it.
pub struct ChildExitReporter {
    sender: mpsc::Sender<ChildEvent>,
    interest: Arc<Mutex<Option<Interest>>>,
}

impl ChildExitReporter {
    pub fn report(&self, event: ChildEvent) -> std::result::Result<(), ()> {
        self.sender.send(event).map_err(|_| ())?;
        if let Some(interest) = self.interest.lock().unwrap().as_ref() {
            interest.poller.post(CompletionPacket::new(interest.event)).ok();
        }
        Ok(())
    }
}

pub struct ChildExitWatcher {
    wait_handle: AtomicPtr<c_void>,
    callback_context: AtomicPtr<c_void>,
    callback_started: Arc<std::sync::atomic::AtomicBool>,
    event_pending: Arc<std::sync::atomic::AtomicBool>,
    event_rx: mpsc::Receiver<ChildEvent>,
    interest: Arc<Mutex<Option<Interest>>>,
    child_handle: AtomicPtr<c_void>,
    pid: Option<NonZeroU32>,
}

impl ChildExitWatcher {
    /// Creates a watcher and takes ownership of `child_handle`.
    ///
    /// The handle remains valid until this watcher is dropped. Callers must
    /// not close it after this function succeeds.
    pub fn new(child_handle: HANDLE) -> Result<ChildExitWatcher, Error> {
        let (event_tx, event_rx) = mpsc::channel();

        let mut wait_handle: HANDLE = ptr::null_mut();
        let interest = Arc::new(Mutex::new(None));
        let callback_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let event_pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sender_ref = Box::new(ChildExitSender {
            sender: event_tx,
            interest: interest.clone(),
            child_handle: AtomicPtr::from(child_handle),
            callback_started: callback_started.clone(),
            event_pending: event_pending.clone(),
        });
        let callback_context = Box::into_raw(sender_ref).cast();

        let success = unsafe {
            RegisterWaitForSingleObject(
                &mut wait_handle,
                child_handle,
                Some(child_exit_callback),
                callback_context,
                INFINITE,
                WT_EXECUTEINWAITTHREAD | WT_EXECUTEONLYONCE,
            )
        };

        if success == 0 {
            unsafe {
                drop(Box::from_raw(callback_context as *mut ChildExitSender));
                CloseHandle(child_handle);
            }
            Err(Error::last_os_error())
        } else {
            let pid = unsafe { NonZeroU32::new(GetProcessId(child_handle)) };
            Ok(ChildExitWatcher {
                event_rx,
                callback_context: AtomicPtr::from(callback_context),
                callback_started,
                event_pending,
                interest,
                pid,
                child_handle: AtomicPtr::from(child_handle),
                wait_handle: AtomicPtr::from(wait_handle),
            })
        }
    }

    /// A watcher for a child this process did not spawn.
    ///
    /// The multiplexer owns the process, so there is nothing here to wait on:
    /// its exit is reported through the returned sender instead. Every handle
    /// field stays null, which is what keeps `Drop` from unregistering a wait
    /// that was never registered or closing a handle this never held.
    pub fn external(pid: Option<NonZeroU32>) -> (ChildExitWatcher, ChildExitReporter) {
        let (event_tx, event_rx) = mpsc::channel();
        let interest = Arc::new(Mutex::new(None));
        let watcher = ChildExitWatcher {
            event_rx,
            callback_context: AtomicPtr::new(ptr::null_mut()),
            callback_started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            event_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            interest: interest.clone(),
            pid,
            child_handle: AtomicPtr::new(ptr::null_mut()),
            wait_handle: AtomicPtr::new(ptr::null_mut()),
        };
        (watcher, ChildExitReporter { sender: event_tx, interest })
    }

    pub fn event_rx(&self) -> &mpsc::Receiver<ChildEvent> {
        &self.event_rx
    }

    pub fn register(&self, poller: &Arc<Poller>, event: Event) {
        let interest = Interest { poller: poller.clone(), event };
        *self.interest.lock().unwrap() = Some(interest);
        if self.event_pending.load(Ordering::Acquire) {
            poller.post(CompletionPacket::new(event)).ok();
        }
    }

    pub fn deregister(&self) {
        *self.interest.lock().unwrap() = None;
    }

    /// Retrieve the Process ID associated to the underlying child process.
    pub fn pid(&self) -> Option<NonZeroU32> {
        self.pid
    }
}

impl Drop for ChildExitWatcher {
    fn drop(&mut self) {
        let wait_handle = self.wait_handle.swap(ptr::null_mut(), Ordering::AcqRel) as HANDLE;
        if !wait_handle.is_null() {
            // Waiting with INVALID_HANDLE_VALUE makes callback cleanup
            // synchronous. Without this, the callback can still be reading
            // the process handle or posting to the poller while both are
            // being torn down.
            unsafe {
                UnregisterWaitEx(wait_handle, INVALID_HANDLE_VALUE);
            }
        }

        let callback_context = self.callback_context.swap(ptr::null_mut(), Ordering::AcqRel);
        if !self.callback_started.load(Ordering::Acquire) && !callback_context.is_null() {
            // A wait that was unregistered before it fired never invokes the
            // callback, so reclaim the context here. If it did fire,
            // `child_exit_callback` already reclaimed it and the synchronous
            // unregister above guarantees that it has finished.
            unsafe {
                drop(Box::from_raw(callback_context as *mut ChildExitSender));
            }
        }

        let child_handle = self.child_handle.swap(ptr::null_mut(), Ordering::AcqRel) as HANDLE;
        if !child_handle.is_null() {
            unsafe {
                CloseHandle(child_handle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::windows::io::IntoRawHandle;
    use std::process::Command;
    use std::sync::Arc;
    use std::time::Duration;

    use super::super::PTY_CHILD_EVENT_TOKEN;
    use super::*;

    #[test]
    pub fn event_is_emitted_when_child_exits() {
        const WAIT_TIMEOUT: Duration = Duration::from_millis(200);

        let poller = Arc::new(Poller::new().unwrap());

        let child = Command::new("cmd.exe").args(["/c", "exit", "1"]).spawn().unwrap();
        let child_handle = child.into_raw_handle() as HANDLE;
        let child_exit_watcher = ChildExitWatcher::new(child_handle).unwrap();
        child_exit_watcher.register(&poller, Event::readable(PTY_CHILD_EVENT_TOKEN));

        // Poll for the event or fail with timeout if nothing has been sent.
        let mut events = polling::Events::new();
        poller.wait(&mut events, Some(WAIT_TIMEOUT)).unwrap();
        assert_eq!(events.iter().next().unwrap().key, PTY_CHILD_EVENT_TOKEN);
        // Verify that at least one `ChildEvent::Exited` was received.
        let expected_status = ExitStatus::from_raw(1);
        assert_eq!(
            child_exit_watcher.event_rx().try_recv(),
            Ok(ChildEvent::Exited(expected_status))
        );
    }

    #[test]
    fn callback_reports_unavailable_exit_status() {
        let (sender, receiver) = mpsc::channel();
        let interest = Arc::new(Mutex::new(None));
        let callback_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let event_pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let context = Box::into_raw(Box::new(ChildExitSender {
            sender,
            interest,
            child_handle: AtomicPtr::from(INVALID_HANDLE_VALUE),
            callback_started: callback_started.clone(),
            event_pending,
        }));

        child_exit_callback(context.cast(), 0);

        assert_eq!(receiver.try_recv(), Ok(ChildEvent::ExitStatusUnavailable));
        assert!(callback_started.load(Ordering::Acquire));
    }

    #[test]
    fn dropping_watcher_closes_the_owned_process_handle() {
        let child = Command::new("cmd.exe").args(["/c", "exit", "0"]).spawn().unwrap();
        let child_handle = child.into_raw_handle() as HANDLE;
        let watcher = ChildExitWatcher::new(child_handle).unwrap();
        drop(watcher);

        let mut flags = 0;
        assert_eq!(unsafe { GetHandleInformation(child_handle, &mut flags) }, 0);
    }
}
