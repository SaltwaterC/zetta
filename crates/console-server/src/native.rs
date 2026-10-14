// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! Native handle ownership and launching a child into a private ConDrv server.
//! Only the child tree launched here enters the kill-on-close job.
use std::ffi::c_void;
use std::io;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::Arc;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::IO::{DeviceIoControl, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Threading::*;

use crate::protocol::*;

pub struct Handle(usize);
impl Handle {
    pub fn new(handle: HANDLE) -> io::Result<Self> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle as usize))
        }
    }
    pub fn duplicate(&self) -> io::Result<Self> {
        let mut handle = null_mut();
        check(unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.raw(),
                GetCurrentProcess(),
                &mut handle,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        })?;
        Self::new(handle)
    }
    pub fn duplicate_owned(&self) -> io::Result<std::os::windows::io::OwnedHandle> {
        use std::os::windows::io::FromRawHandle;
        let handle = self.duplicate()?;
        let raw = handle.raw();
        std::mem::forget(handle);
        Ok(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw) })
    }
    pub fn raw(&self) -> HANDLE {
        self.0 as HANDLE
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.raw());
        }
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

pub fn open_device(parent: HANDLE, path: &str, access: u32, options: u32) -> io::Result<Handle> {
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
    // All pointers remain valid until the synchronous NtOpenFile returns.
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
    Handle::new(handle)
}

thread_local! {
    static IO_EVENT: std::cell::RefCell<Option<Handle>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn ioctl(
    server: &Handle,
    code: u32,
    input: *const c_void,
    input_size: usize,
    output: *mut c_void,
    output_size: usize,
) -> io::Result<u32> {
    IO_EVENT.with(|cached| {
        let mut cached = cached.borrow_mut();
        if cached.is_none() {
            *cached = Some(Handle::new(unsafe { CreateEventW(null(), 1, 0, null()) })?);
        }
        let event = cached.as_ref().unwrap();
        check(unsafe { ResetEvent(event.raw()) })?;
        let mut written = 0;
        // Each caller has an independent event: pending READ_IO must not share
        // completion signaling with the input worker's COMPLETE_IO.
        let mut overlapped: OVERLAPPED = unsafe { zeroed() };
        overlapped.hEvent = event.raw();
        // Buffers and OVERLAPPED remain live until GetOverlappedResult completes.
        let ok = unsafe {
            DeviceIoControl(
                server.raw(),
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
            check(unsafe { GetOverlappedResult(server.raw(), &overlapped, &mut written, 1) })?;
        }
        Ok(written)
    })
}

pub fn transfer(
    server: &Handle,
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
    ioctl(
        server,
        code,
        (&operation as *const Operation).cast(),
        size_of::<Operation>(),
        null_mut(),
        0,
    )?;
    Ok(())
}

pub struct Console {
    pub server: Handle,
    pub reference: Handle,
    // The driver retains a reference to this event; keep our handle for the session.
    pub _input_event: Handle,
}
impl Console {
    pub fn new() -> io::Result<Self> {
        let server = open_device(null_mut(), r"\Device\ConDrv\Server", GENERIC_ALL, 0)?;
        let reference = open_device(
            server.raw(),
            r"\Reference",
            GENERIC_READ | GENERIC_WRITE | 0x00100000,
            0x20,
        )?;
        let input_event = Handle::new(unsafe { CreateEventW(null(), 1, 0, null()) })?;
        let raw_event = input_event.raw();
        ioctl(
            &server,
            SET_SERVER_INFORMATION,
            (&raw_event as *const HANDLE).cast(),
            size_of::<HANDLE>(),
            null_mut(),
            0,
        )?;
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
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
        }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
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
    pub process: Handle,
    pub job: Arc<Handle>,
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
            || launch.environment.iter().any(|(name, value)| {
                name.is_empty() || name.contains(['=', '\0']) || value.contains('\0')
            })
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
        env_block.push(0);
        let directory: Option<Vec<u16>> = launch
            .directory
            .as_ref()
            .map(|path| path.as_os_str().encode_wide().chain([0]).collect());
        let job = Arc::new(Handle::new(unsafe { CreateJobObjectW(null(), null()) })?);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = match scope {
            JobScope::Tree => JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JobScope::Child => {
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK
            }
        };
        check(unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of_val(&limits) as u32,
            )
        })?;
        let mut host_process = null_mut();
        check(unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                GetCurrentProcess(),
                GetCurrentProcess(),
                &mut host_process,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        })?;
        let host_process = Handle::new(host_process)?;
        let mut pty = PseudoConsole {
            signal: null_mut(),
            reference: console.reference.raw(),
            process: host_process.raw(),
        };
        let mut attributes = AttributeList::new()?;
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
        let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        // Null standard handles are replaced by this new console, rather than
        // inheriting the harness's redirected stdout/stderr.
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.lpAttributeList = attributes.raw();
        let mut info: PROCESS_INFORMATION = unsafe { zeroed() };
        let mut wide: Vec<u16> = command.encode_utf16().chain([0]).collect();
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
        let process = Handle::new(info.hProcess)?;
        let thread = Handle::new(info.hThread)?;
        if unsafe { AssignProcessToJobObject(job.raw(), process.raw()) } == 0 {
            let error = io::Error::last_os_error();
            unsafe {
                TerminateProcess(process.raw(), 1);
            }
            return Err(error);
        }
        if unsafe { ResumeThread(thread.raw()) } == u32::MAX {
            let error = io::Error::last_os_error();
            unsafe {
                TerminateJobObject(job.raw(), 1);
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
        let wait = unsafe { WaitForSingleObject(self.process.raw(), 5000) };
        if wait != WAIT_OBJECT_0 {
            return Err(io::Error::other(
                "child did not exit after console disconnect",
            ));
        }
        let mut code = 0;
        check(unsafe { GetExitCodeProcess(self.process.raw(), &mut code) })?;
        Ok(code)
    }
}
/// Ends a console client that outlived its console's close grace period.
pub fn terminate_process(pid: u32) {
    let process = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if !process.is_null() {
        unsafe {
            TerminateProcess(process, 1);
            CloseHandle(process);
        }
    }
}
fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
