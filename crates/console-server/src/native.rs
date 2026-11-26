//! The Windows objects the server works through: owned handles, the ConDrv
//! device and its IOCTLs, and launching the child inside a kill-on-close job.

// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! Native handle ownership and launching a child into a private ConDrv server.
//! Only the child tree launched here enters the kill-on-close job.
use std::ffi::c_void;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::ptr::{null, null_mut};
use std::sync::Arc;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::IO::{DeviceIoControl, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Threading::*;

use crate::protocol::*;

/// Takes ownership of a handle a Win32 call just returned, or reports the
/// thread's last error when the call returned null or `INVALID_HANDLE_VALUE`
/// for failure.
///
/// # Safety
///
/// `handle` is null, `INVALID_HANDLE_VALUE`, or an open handle that nothing
/// else owns or closes.
pub unsafe fn adopt(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: the caller hands over an open handle nobody else owns.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

/// A manual-reset event, unnamed and not inheritable.
pub struct Event(OwnedHandle);
impl Event {
    pub fn new() -> io::Result<Self> {
        // SAFETY: null security attributes and name are allowed, and the call
        // returns null or a new handle nothing else owns.
        unsafe { adopt(CreateEventW(null(), 1, 0, null())) }.map(Self)
    }
    pub fn set(&self) {
        // SAFETY: the event is open for as long as `self` is borrowed. Setting
        // an open event cannot fail.
        unsafe { SetEvent(self.raw()) };
    }
    pub fn reset(&self) {
        // SAFETY: as in `set`.
        unsafe { ResetEvent(self.raw()) };
    }
    pub fn raw(&self) -> HANDLE {
        self.0.as_raw_handle()
    }
    pub fn try_clone(&self) -> io::Result<Self> {
        self.0.try_clone().map(Self)
    }
}
impl From<Event> for OwnedHandle {
    fn from(event: Event) -> Self {
        event.0
    }
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}
#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: HANDLE,
    name: *mut UnicodeString,
    attributes: u32,
    security: *mut c_void,
    qos: *mut c_void,
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtOpenFile(
        handle: *mut HANDLE,
        access: u32,
        attributes: *mut ObjectAttributes,
        status: *mut IoStatus,
        share: u32,
        options: u32,
    ) -> i32;
}

pub fn open_device(
    parent: HANDLE,
    path: &str,
    access: u32,
    options: u32,
) -> io::Result<OwnedHandle> {
    let mut wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    let mut name = UnicodeString {
        length: ((wide.len() - 1) * 2) as u16,
        maximum_length: (wide.len() * 2) as u16,
        buffer: wide.as_mut_ptr(),
    };
    let mut attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root: parent,
        name: &mut name,
        attributes: 0x40,
        security: null_mut(),
        qos: null_mut(),
    };
    let mut status = IoStatus::default();
    let mut handle = null_mut();
    // SAFETY: every pointer (handle, attributes and the name and wide buffer
    // they point to, status) is a live local that outlives the synchronous
    // NtOpenFile, and the UNICODE_STRING lengths match `wide`.
    let result = unsafe {
        NtOpenFile(
            &mut handle,
            access,
            &mut attributes,
            &mut status,
            7,
            options,
        )
    };
    if result < 0 {
        return Err(io::Error::other(format!(
            "NtOpenFile({path}): NTSTATUS {result:#010x}"
        )));
    }
    // SAFETY: NtOpenFile succeeded, so `handle` is a new handle nothing else
    // owns.
    unsafe { adopt(handle) }
}

thread_local! {
    static IO_EVENT: std::cell::RefCell<Option<Event>> = const { std::cell::RefCell::new(None) };
}

/// Sends `code` to the driver and waits for it to finish.
///
/// # Safety
///
/// `input` is null or readable for `input_size` bytes, and `output` null or
/// writable for `output_size` bytes, and so is every buffer the request they
/// describe points the driver at, until this returns.
unsafe fn ioctl(
    server: &OwnedHandle,
    code: u32,
    input: *const c_void,
    input_size: usize,
    output: *mut c_void,
    output_size: usize,
) -> io::Result<u32> {
    IO_EVENT.with(|cached| {
        let mut cached = cached.borrow_mut();
        if cached.is_none() {
            *cached = Some(Event::new()?);
        }
        let event = cached.as_ref().unwrap();
        event.reset();
        let mut written = 0;
        // Each caller has an independent event: pending READ_IO must not share
        // completion signaling with the input worker's COMPLETE_IO.
        // SAFETY: OVERLAPPED is a plain C struct; all-zero is its documented
        // initial state.
        let mut overlapped: OVERLAPPED = unsafe { zeroed() };
        overlapped.hEvent = event.raw();
        // SAFETY: server is open while borrowed; the caller guarantees `input`
        // and `output` and what they point at, and `written` and `overlapped`
        // stay live until the waiting GetOverlappedResult below has completed
        // the IO.
        let ok = unsafe {
            DeviceIoControl(
                server.as_raw_handle(),
                code,
                input,
                input_size as u32,
                output,
                output_size as u32,
                &mut written,
                &mut overlapped,
            )
        };
        if ok == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(error);
            }
            // SAFETY: overlapped is the one this pending IO was issued with,
            // and waiting (bWait = 1) keeps it and the buffers in use only
            // until this call returns.
            check(unsafe {
                GetOverlappedResult(server.as_raw_handle(), &overlapped, &mut written, 1)
            })?;
        }
        Ok(written)
    })
}

/// Completes a request with `completion`'s status and reply.
pub(crate) fn complete_io(server: &OwnedHandle, completion: &Completion<'_>) -> io::Result<()> {
    // SAFETY: the completion is readable for its size, and the reply buffer
    // it points the driver at is borrowed by it for as long as it lives.
    unsafe {
        ioctl(
            server,
            COMPLETE_IO,
            std::ptr::from_ref(completion).cast(),
            size_of::<Completion>(),
            null_mut(),
            0,
        )
    }
    .map(drop)
}

/// Completes the previous request, if `completion` is one, and reads the next
/// into `packet`. Returns how many bytes of `packet` the driver filled; a
/// single call saves a round trip per request.
pub(crate) fn read_io(
    server: &OwnedHandle,
    completion: Option<&Completion<'_>>,
    packet: &mut Packet,
) -> io::Result<usize> {
    let (input, input_size) = completion.map_or((null(), 0), |completion| {
        (
            std::ptr::from_ref(completion).cast(),
            size_of::<Completion>(),
        )
    });
    // SAFETY: the completion, if any, is readable for its size and borrows
    // its reply buffer, and `packet` is writable for its size; all outlive
    // the call.
    let received = unsafe {
        ioctl(
            server,
            READ_IO,
            input,
            input_size,
            std::ptr::from_mut(packet).cast(),
            size_of::<Packet>(),
        )
    }?;
    Ok(received as usize)
}

pub fn transfer(
    server: &OwnedHandle,
    code: u32,
    id: Identifier,
    offset: u32,
    bytes: &mut [u8],
) -> io::Result<()> {
    let operation = Operation {
        id,
        buffer: Buffer {
            data: bytes.as_mut_ptr().cast(),
            size: bytes.len() as u32,
            offset,
        },
    };
    // SAFETY: the operation is readable for its size, and the buffer it points
    // the driver at is `bytes`, borrowed for the whole call.
    unsafe {
        ioctl(
            server,
            code,
            std::ptr::from_ref(&operation).cast(),
            size_of::<Operation>(),
            null_mut(),
            0,
        )
    }?;
    Ok(())
}

pub struct Console {
    pub server: OwnedHandle,
    pub reference: OwnedHandle,
    // The driver retains a reference to this event; keep our handle for the session.
    pub _input_event: Event,
}
impl Console {
    pub fn new() -> io::Result<Self> {
        let server = open_device(null_mut(), r"\Device\ConDrv\Server", GENERIC_ALL, 0)?;
        let reference = open_device(
            server.as_raw_handle(),
            r"\Reference",
            GENERIC_READ | GENERIC_WRITE | 0x00100000,
            0x20,
        )?;
        let input_event = Event::new()?;
        let raw_event = input_event.raw();
        // SAFETY: the input is one HANDLE, readable for its size, and it
        // points the driver at nothing else.
        unsafe {
            ioctl(
                &server,
                SET_SERVER_INFORMATION,
                std::ptr::from_ref(&raw_event).cast(),
                size_of::<HANDLE>(),
                null_mut(),
                0,
            )
        }?;
        Ok(Self {
            server,
            reference,
            _input_event: input_event,
        })
    }
}

#[repr(C)]
struct PseudoConsole {
    signal: HANDLE,
    reference: HANDLE,
    process: HANDLE,
}

struct AttributeList {
    storage: Vec<usize>,
}
impl AttributeList {
    fn new() -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: a null list asks only for the required size, written to the
        // local `bytes`.
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
        }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        // SAFETY: storage is pointer-aligned and at least `bytes` long, the
        // size the query above asked for one attribute.
        check(unsafe {
            InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 1, 0, &mut bytes)
        })?;
        Ok(Self { storage })
    }
    fn raw(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
}
impl Drop for AttributeList {
    fn drop(&mut self) {
        // SAFETY: an AttributeList exists only once
        // InitializeProcThreadAttributeList succeeded on its storage, and it
        // is deleted exactly once, here.
        unsafe {
            DeleteProcThreadAttributeList(self.raw());
        }
    }
}

/// What the launched child's kill-on-close job holds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JobScope {
    /// The child and everything it starts: the probe's watchdog ends it all.
    Tree,
    /// Only the child: what it starts breaks away silently, so a program
    /// launched from a shell outlives the shell's console, as with ConPTY.
    Child,
}

pub struct Child {
    pub process: OwnedHandle,
    pub job: Arc<OwnedHandle>,
    pub pid: u32,
}
impl Child {
    pub fn spawn(console: &Console, command: &str) -> io::Result<Self> {
        Self::spawn_with_launch(
            console,
            &crate::Launch {
                command: command.to_owned(),
                environment: Default::default(),
                directory: None,
                ..Default::default()
            },
            JobScope::Tree,
        )
    }
    pub fn spawn_with_launch(
        console: &Console,
        launch: &crate::Launch,
        scope: JobScope,
    ) -> io::Result<Self> {
        let command = &launch.command;
        if command.contains('\0')
            || launch
                .environment
                .iter()
                .any(|(name, value)| !valid_environment_name(name) || value.contains('\0'))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid command or environment entry",
            ));
        }
        let mut environment: std::collections::BTreeMap<
            String,
            (std::ffi::OsString, std::ffi::OsString),
        > = std::env::vars_os()
            .map(|(name, value)| (name.to_string_lossy().to_uppercase(), (name, value)))
            .collect();
        for (name, value) in &launch.environment {
            environment.insert(name.to_uppercase(), (name.into(), value.into()));
        }
        use std::os::windows::ffi::OsStrExt;
        let mut env_block: Vec<u16> = environment
            .values()
            .flat_map(|(name, value)| {
                name.encode_wide()
                    .chain([61])
                    .chain(value.encode_wide())
                    .chain([0])
            })
            .collect();
        // The block ends with an empty string, so an empty one is two NULs.
        if env_block.is_empty() {
            env_block.push(0);
        }
        env_block.push(0);
        let directory: Option<Vec<u16>> = launch
            .directory
            .as_ref()
            .map(|path| path.as_os_str().encode_wide().chain([0]).collect());
        // SAFETY: null security attributes and name are allowed, and the call
        // returns null or a new handle nothing else owns.
        let job = Arc::new(unsafe { adopt(CreateJobObjectW(null(), null())) }?);
        // SAFETY: the limit struct is plain integers; all-zero means no limits.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = match scope {
            JobScope::Tree => JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JobScope::Child => {
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK
            }
        };
        // SAFETY: job is the owned job handle, and the pointer and length
        // describe `limits`, the struct this information class expects.
        check(unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of_val(&limits) as u32,
            )
        })?;
        // A real handle to this process, which the pseudo-handle is not.
        // SAFETY: the current-process pseudo-handle is always valid and needs
        // no closing.
        let host_process =
            unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(GetCurrentProcess()) }
                .try_clone_to_owned()?;
        let mut pty = PseudoConsole {
            signal: null_mut(),
            reference: console.reference.as_raw_handle(),
            process: host_process.as_raw_handle(),
        };
        let mut attributes = AttributeList::new()?;
        // SAFETY: attributes was initialized for one attribute. The value is
        // a pointer to `pty`, which is what an HPCON is, with HPCON's size;
        // the list keeps only that pointer, and `pty` outlives both
        // `attributes` (declared after it) and the CreateProcessW below.
        check(unsafe {
            UpdateProcThreadAttribute(
                attributes.raw(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                (&mut pty as *mut PseudoConsole).cast(),
                size_of::<HANDLE>(),
                null_mut(),
                null_mut(),
            )
        })?;
        // SAFETY: STARTUPINFOEXW is a plain C struct for which all-zero (null
        // pointers and handles) is valid.
        let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        // Null standard handles are replaced by this new console, rather than
        // inheriting the harness's redirected stdout/stderr.
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.lpAttributeList = attributes.raw();
        // SAFETY: PROCESS_INFORMATION is handles and integers; all-zero is valid.
        let mut info: PROCESS_INFORMATION = unsafe { zeroed() };
        let mut wide: Vec<u16> = command.encode_utf16().chain([0]).collect();
        // SAFETY: `wide` is a writable NUL-terminated command line, the
        // environment block is NUL-separated UTF-16 ending in an extra NUL
        // (CREATE_UNICODE_ENVIRONMENT), the directory is NUL-terminated or
        // null, `startup` is an initialized STARTUPINFOEXW with a live
        // attribute list (EXTENDED_STARTUPINFO_PRESENT), and all of them
        // outlive the call.
        check(unsafe {
            CreateProcessW(
                null(),
                wide.as_mut_ptr(),
                null(),
                null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
                env_block.as_mut_ptr().cast(),
                directory.as_ref().map_or(null(), |wide| wide.as_ptr()),
                &startup.StartupInfo,
                &mut info,
            )
        })?;
        // SAFETY: CreateProcessW succeeded, so both are new handles this
        // function alone owns.
        let (process, thread) = unsafe {
            (
                OwnedHandle::from_raw_handle(info.hProcess),
                OwnedHandle::from_raw_handle(info.hThread),
            )
        };
        if let Err(error) = crate::launch_control::enable_ctrl_c(process.as_raw_handle()) {
            // SAFETY: process is the owned handle of the child just created.
            unsafe {
                TerminateProcess(process.as_raw_handle(), 1);
            }
            return Err(error);
        }
        // SAFETY: job and process are owned handles opened above.
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle()) } == 0 {
            let error = io::Error::last_os_error();
            // SAFETY: process is the owned handle of the child just created.
            unsafe {
                TerminateProcess(process.as_raw_handle(), 1);
            }
            return Err(error);
        }
        // SAFETY: thread is the owned handle of the child's suspended thread.
        if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
            let error = io::Error::last_os_error();
            // SAFETY: job is the owned job handle the child was assigned to.
            unsafe {
                TerminateJobObject(job.as_raw_handle(), 1);
            }
            return Err(error);
        }
        Ok(Self {
            process,
            job,
            pid: info.dwProcessId,
        })
    }
    pub fn exit_code(&self) -> io::Result<u32> {
        // SAFETY: self.process is owned by self and open while it is borrowed.
        let wait = unsafe { WaitForSingleObject(self.process.as_raw_handle(), 5000) };
        if wait != WAIT_OBJECT_0 {
            return Err(io::Error::other(
                "child did not exit after console disconnect",
            ));
        }
        let mut code = 0;
        // SAFETY: as above, and `code` is a writable local.
        check(unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) })?;
        Ok(code)
    }
}
/// Ends a console client that outlived its console's close grace period.
pub fn terminate_process(pid: u32) {
    // SAFETY: OpenProcess takes only integers, and returns null or a new
    // handle nothing else owns.
    if let Ok(process) = unsafe { adopt(OpenProcess(PROCESS_TERMINATE, 0, pid)) } {
        // SAFETY: `process` is open until it is dropped, after this call.
        unsafe { TerminateProcess(process.as_raw_handle(), 1) };
    }
}
fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Windows inherits internal names such as `=C:` (a drive's current
/// directory) and CMD's `=ExitCode`. Their leading equals sign is part of
/// the name; additional equals signs would corrupt an environment entry.
fn valid_environment_name(name: &str) -> bool {
    let name = name.strip_prefix('=').unwrap_or(name);
    !name.is_empty() && !name.contains(['=', '\0'])
}

#[cfg(test)]
#[path = "tests/native.rs"]
mod tests;
