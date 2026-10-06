//! Keeps a macOS, Windows or Linux host from idle-sleeping while a client is
//! talking to it.
//!
//! A Mac idle-sleeps with its lid open: once the display is off and the
//! `pmset sleep` timer runs out, it sleeps regardless of what its daemons are
//! doing. An SSH session survives that because powerd holds a tty assertion
//! for an active login, and because the Wi-Fi chip wakes the host for TCP
//! services it advertises, so the next SSH connection brings it back. Neither
//! applies to this server: it holds no assertion, and a datagram to its UDP
//! port wakes nothing. The session freezes on the client's "Last contact"
//! banner until something else wakes the Mac, at which point it resumes as if
//! nothing had happened. That is the symptom this module exists for.
//!
//! The assertion is `NetworkClientActive`, the type powerd itself takes for an
//! active SSH tty and the one IOKit documents for a process serving remote
//! clients. `PreventUserIdleSystemSleep` (what `caffeinate -i` takes) is not
//! enough: it only stops idle sleep from full wake, and a Mac woken by the SSH
//! bootstrap that launched this server may be in dark wake, which it leaves
//! for maintenance sleep regardless. On AC power the assertion holds in dark
//! or full wake; on battery it only prevents idle sleep. Neither stops a lid
//! close or an explicit sleep, the same as an SSH session.
//!
//! Windows has the same failure with a different mechanism: the idle timer
//! counts only local input, so a laptop sleeps (S3, or Modern Standby's
//! low-power state) under a live remote session, and an inbound datagram does
//! not wake it. Windows OpenSSH holds no power request for its sessions
//! either. There the guard holds a `PowerRequestSystemRequired` power request,
//! which `powercfg /requests` lists under the reason string below. It
//! prevents idle sleep on AC and on battery; like the macOS assertion, it does
//! not stop a lid close, the power button or an explicit sleep. A power
//! request rather than `SetThreadExecutionState`, because the latter belongs
//! to the calling thread and is released when that thread exits, where a
//! request is a handle that lives exactly as long as the guard.
//!
//! Linux, likewise: GNOME and KDE suspend an idle laptop under a live remote
//! session, and sshd takes nothing to stop them. There the guard holds a
//! systemd-logind `sleep` inhibitor in `block` mode, which `systemd-inhibit
//! --list` shows under the same reason. Idle suspend reaches logind as a
//! `Suspend` call, which the inhibitor refuses. Lid close is not stopped,
//! because logind's `LidSwitchIgnoreInhibited` defaults to on. Unlike the
//! other platforms an explicit suspend is refused too, unless the caller may
//! ignore inhibitors (root, or `systemctl suspend -i`): that is what a block
//! inhibitor means, and logind offers no idle-only alternative that the
//! desktops honour. `idle` is deliberately not inhibited, as that would also
//! stop the screen locking on an unattended laptop. A host without logind
//! (no systemd, or a container without the system bus) fails to acquire,
//! which is reported with `-v` and otherwise ignored.
//!
//! It is held only while the peer is present, as IOKit asks, so a detached
//! session whose client has gone does not keep a laptop awake for the life of
//! the session. Every other platform gets a guard that does nothing.

use std::time::Duration;

/// How long the peer may go unheard before the host is allowed to sleep.
///
/// Much longer than `KEEP_ALIVE_LINGER`, because the costs run the other
/// way. Holding the assertion a minute too long costs a minute of being
/// awake. Releasing it too early during a client-side outage (a Wi-Fi roam, a
/// suspended client laptop that resumes quickly) lets a Mac whose idle timer
/// has long since expired sleep at once, and then the client's returning
/// datagrams cannot wake it.
pub const PRESENCE_LINGER: Duration = Duration::from_secs(60);

/// Whether the peer counts as attached for the purpose of keeping the host
/// awake: an authenticated client that has been heard from recently.
pub fn peer_present(associated: bool, since_recv: Duration) -> bool {
    associated && since_recv < PRESENCE_LINGER
}

/// Holds the platform's idle-sleep assertion while the peer is present.
///
/// Dropping the guard releases the assertion. The kernel also releases it
/// when the process exits, so a crash cannot leave the host pinned awake.
pub struct IdleSleepGuard {
    present: bool,
    assertion: Option<platform::Assertion>,
}

impl IdleSleepGuard {
    pub fn new() -> Self {
        Self {
            present: false,
            assertion: None,
        }
    }

    /// Take or release the assertion if `present` changed since the last
    /// call. Called once per server loop pass, so the unchanged case is a
    /// comparison and nothing else. A failed acquisition is retried on the
    /// next arrival rather than every pass, so it cannot spin.
    pub fn update(&mut self, present: bool, verbose: bool) {
        if present == self.present {
            return;
        }
        self.present = present;
        if present {
            match platform::Assertion::acquire() {
                Ok(assertion) => self.assertion = assertion,
                Err(error) if verbose => {
                    eprintln!("zosh-server-rs: could not prevent idle sleep: {error}");
                }
                Err(_) => {}
            }
        } else {
            self.assertion = None;
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{c_char, c_void};

    type CFStringRef = *const c_void;
    type IOPMAssertionID = u32;
    type IOReturn = i32;

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const K_IOPM_ASSERTION_LEVEL_ON: u32 = 255;
    const K_IO_RETURN_SUCCESS: IOReturn = 0;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            level: u32,
            name: CFStringRef,
            assertion_id: *mut IOPMAssertionID,
        ) -> IOReturn;
        fn IOPMAssertionRelease(assertion_id: IOPMAssertionID) -> IOReturn;
    }

    pub struct Assertion(IOPMAssertionID);

    impl Assertion {
        pub fn acquire() -> Result<Option<Self>, String> {
            let kind = cf_string(c"NetworkClientActive")?;
            let name = match cf_string(c"zosh-server: Mosh client attached") {
                Ok(name) => name,
                Err(error) => {
                    unsafe { CFRelease(kind) };
                    return Err(error);
                }
            };
            let mut id: IOPMAssertionID = 0;
            let result = unsafe {
                IOPMAssertionCreateWithName(kind, K_IOPM_ASSERTION_LEVEL_ON, name, &mut id)
            };
            unsafe {
                CFRelease(name);
                CFRelease(kind);
            }
            if result != K_IO_RETURN_SUCCESS {
                return Err(format!("IOPMAssertionCreateWithName returned {result:#x}"));
            }
            Ok(Some(Self(id)))
        }
    }

    impl Drop for Assertion {
        fn drop(&mut self) {
            unsafe { IOPMAssertionRelease(self.0) };
        }
    }

    fn cf_string(value: &std::ffi::CStr) -> Result<CFStringRef, String> {
        let string = unsafe {
            CFStringCreateWithCString(std::ptr::null(), value.as_ptr(), K_CF_STRING_ENCODING_UTF8)
        };
        if string.is_null() {
            return Err(format!("CFStringCreateWithCString failed for {value:?}"));
        }
        Ok(string)
    }
}

#[cfg(windows)]
mod platform {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Power::{
        PowerClearRequest, PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest,
    };
    use windows::Win32::System::Threading::{
        POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
    };
    use windows::core::PWSTR;

    /// `POWER_REQUEST_CONTEXT_VERSION`, which the bindings do not export.
    const POWER_REQUEST_CONTEXT_VERSION: u32 = 0;

    pub struct Assertion(HANDLE);

    impl Assertion {
        pub fn acquire() -> Result<Option<Self>, String> {
            // The reason is copied by `PowerCreateRequest`, so it only has to
            // outlive the call.
            let mut reason: Vec<u16> = "zosh-server: Mosh client attached"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let context = REASON_CONTEXT {
                Version: POWER_REQUEST_CONTEXT_VERSION,
                Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
                Reason: REASON_CONTEXT_0 {
                    SimpleReasonString: PWSTR(reason.as_mut_ptr()),
                },
            };
            let request = unsafe { PowerCreateRequest(&context) }
                .map_err(|error| format!("PowerCreateRequest failed: {error}"))?;
            if let Err(error) = unsafe { PowerSetRequest(request, PowerRequestSystemRequired) } {
                unsafe {
                    let _ = CloseHandle(request);
                }
                return Err(format!("PowerSetRequest failed: {error}"));
            }
            Ok(Some(Self(request)))
        }
    }

    impl Drop for Assertion {
        fn drop(&mut self) {
            // Closing the handle also clears the request; clearing first
            // states the intent rather than relying on it.
            unsafe {
                let _ = PowerClearRequest(self.0, PowerRequestSystemRequired);
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::os::fd::OwnedFd;
    use std::time::Duration;

    /// How long acquisition may hold up the server loop. It runs on the loop
    /// thread, once per client arrival, and logind normally answers in about
    /// a millisecond; this bounds the damage of a wedged logind to one stall.
    const LOGIND_TIMEOUT: Duration = Duration::from_secs(2);

    /// The inhibitor lasts exactly as long as logind's end of this descriptor
    /// stays open, so holding it is the whole of the lock and closing it (on
    /// drop, or on exit) releases it.
    pub struct Assertion {
        _lock: OwnedFd,
    }

    impl Assertion {
        pub fn acquire() -> Result<Option<Self>, String> {
            // The connection is only needed for the call: the descriptor
            // outlives it.
            let connection = zbus::blocking::connection::Builder::system()
                .and_then(|builder| builder.method_timeout(LOGIND_TIMEOUT).build())
                .map_err(|error| format!("connecting to the system bus: {error}"))?;
            let reply = connection
                .call_method(
                    Some("org.freedesktop.login1"),
                    "/org/freedesktop/login1",
                    Some("org.freedesktop.login1.Manager"),
                    "Inhibit",
                    &("sleep", "zosh-server", "Mosh client attached", "block"),
                )
                .map_err(|error| format!("logind Inhibit failed: {error}"))?;
            let lock: zbus::zvariant::OwnedFd = reply
                .body()
                .deserialize()
                .map_err(|error| format!("logind Inhibit replied unexpectedly: {error}"))?;
            Ok(Some(Self { _lock: lock.into() }))
        }
    }
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
mod platform {
    /// No assertion is taken on this platform.
    pub struct Assertion;

    impl Assertion {
        pub fn acquire() -> Result<Option<Self>, String> {
            Ok(None)
        }
    }
}

#[cfg(test)]
#[path = "tests/sleep_guard.rs"]
mod tests;
