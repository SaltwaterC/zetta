//! The Windows Terminal handoff COM server: the class Windows activates when
//! a console application starts with Zetta set as the default terminal, and
//! the duplication of the console's handles into a request the application
//! opens as a tab.
//!
//! The three `ITerminalHandoff` interfaces and `TerminalStartupInfo` keep the
//! names and parameter lists Windows defines them with, in the `abi` module;
//! everything Zetta writes around them passes the console's handles as one
//! `ConsoleHandles`.

use super::*;

/// The handoff ABI as Windows declares it: the startup record and the three
/// interface versions, with Windows' names and parameter lists. A module of
/// their own so the lints those names and lists trip are suppressed for them
/// alone; the `#[interface]` macro accepts no attribute on the traits it
/// expands.
mod abi {
    #![expect(
        non_snake_case,
        reason = "field and method names are the Windows Terminal handoff ABI's"
    )]
    #![expect(
        clippy::too_many_arguments,
        reason = "`ITerminalHandoff2` and `ITerminalHandoff3` pass seven values; \
                  the vtable layout is Windows', so they cannot be bundled"
    )]

    use super::*;

    /// The strings are BSTRs the caller owns, as for any `[in]` argument: held
    /// in `ManuallyDrop` so they are read without ever being freed here.
    #[repr(C)]
    pub(super) struct TerminalStartupInfo {
        pub(super) pszTitle: std::mem::ManuallyDrop<windows_core::BSTR>,
        pub(super) pszIconPath: *mut u16,
        pub(super) iconIndex: i32,
        pub(super) dwX: u32,
        pub(super) dwY: u32,
        pub(super) dwXSize: u32,
        pub(super) dwYSize: u32,
        pub(super) dwXCountChars: u32,
        pub(super) dwYCountChars: u32,
        pub(super) dwFillAttribute: u32,
        pub(super) dwFlags: u32,
        pub(super) wShowWindow: u16,
    }

    #[interface("59D55CCE-FC8A-48B4-ACE8-0A9286C6557F")]
    pub(super) unsafe trait ITerminalHandoff: windows_core::IUnknown {
        fn EstablishPtyHandoff(
            &self,
            input: HANDLE,
            output: HANDLE,
            signal: HANDLE,
            reference: HANDLE,
            server: HANDLE,
            client: HANDLE,
        ) -> HRESULT;
    }

    #[interface("AA6B364F-4A50-4176-9002-0AE755E7B5EF")]
    pub(super) unsafe trait ITerminalHandoff2: windows_core::IUnknown {
        fn EstablishPtyHandoff(
            &self,
            input: HANDLE,
            output: HANDLE,
            signal: HANDLE,
            reference: HANDLE,
            server: HANDLE,
            client: HANDLE,
            startup: TerminalStartupInfo,
        ) -> HRESULT;
    }

    #[interface("6F23DA90-15C5-4203-9DB0-64E73F1B1B00")]
    pub(super) unsafe trait ITerminalHandoff3: windows_core::IUnknown {
        fn EstablishPtyHandoff(
            &self,
            input: *mut HANDLE,
            output: *mut HANDLE,
            signal: HANDLE,
            reference: HANDLE,
            server: HANDLE,
            client: HANDLE,
            startup: *const TerminalStartupInfo,
        ) -> HRESULT;
    }
}
use abi::*;

/// The part of the Windows Terminal handoff's startup metadata Zetta applies.
///
/// `TerminalStartupInfo` keeps the ABI layout in step; only the fields Zetta
/// acts on are decoded out of it. The icon, window geometry and show state the
/// caller also sends are left in the raw struct until something applies them.
#[derive(Clone, Debug)]
pub(crate) struct HandoffStartupMetadata {
    pub(crate) title: Option<String>,
}

/// The six handles are duplicated while the COM call is active. The handoff
/// provider owns the originals and is allowed to close them as soon as the
/// method returns, so retaining the duplicates is part of the protocol rather
/// than an optimization.
pub(crate) struct WindowsHandoffRequest {
    pub(crate) conin: OwnedHandle,
    pub(crate) conout: OwnedHandle,
    pub(crate) signal: OwnedHandle,
    pub(crate) reference: OwnedHandle,
    pub(crate) server: OwnedHandle,
    pub(crate) client: OwnedHandle,
    pub(crate) child_pid: u32,
    pub(crate) startup: Option<HandoffStartupMetadata>,
}

impl WindowsHandoffRequest {
    pub(crate) fn duplicate_child_handle(&self) -> std::io::Result<OwnedHandle> {
        self.client.try_clone()
    }

    pub(crate) fn into_handover(self) -> PtyHandover {
        let control = Arc::new(WindowsHandoffControl {
            signal: Mutex::new(std::fs::File::from(self.signal)),
            _keep_open: [self.reference, self.server, self.client],
        });
        PtyHandover {
            conout: self.conout,
            conin: self.conin,
            child_pid: self.child_pid,
            replay: Vec::new(),
            control,
        }
    }
}

pub(crate) fn monitor_handoff_child(
    handle: OwnedHandle,
    mut child_events: terminal::AttachedChildEvents,
) {
    thread::Builder::new()
        .name("zetta-handoff-child".to_owned())
        .spawn(move || {
            let process = HANDLE(handle.as_raw_handle());
            // SAFETY: `handle` is the duplicated client process handle this
            // closure owns, so it stays open for the wait.
            unsafe {
                WaitForSingleObject(process, INFINITE);
            }
            let mut exit_code = 0;
            // SAFETY: `process` is still owned by `handle`, and `exit_code` is
            // a writable u32 for the duration of the call.
            if unsafe { GetExitCodeProcess(process, &mut exit_code) }.is_ok() {
                let _ = child_events.report_exit(exit_code as i32);
            } else {
                let _ = child_events.report_status_unavailable();
            }
        })
        .ok();
}

/// Only `signal` is ever read. The handoff's reference, server and client
/// handles are held in `_keep_open` so the console they belong to stays open
/// for as long as the handover does: dropping them early closes the client's
/// end of the protocol, so the field is the hold itself rather than state.
struct WindowsHandoffControl {
    /// Locked so the packets of two resizes cannot interleave.
    signal: Mutex<std::fs::File>,
    _keep_open: [OwnedHandle; 3],
}

impl PtyControl for WindowsHandoffControl {
    fn resize(&self, columns: u16, lines: u16) {
        // This is the private signal packet used by Microsoft's ConPTY host.
        // The signal handle is the host-side pipe duplicated during handoff;
        // keeping the write here preserves resizing after the session moves
        // into Zetta rather than merely retaining the lifetime handles.
        use std::io::Write as _;

        let mut packet = [0_u8; 6];
        for (bytes, value) in packet
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip([8_u16, columns, lines])
        {
            *bytes = value.to_le_bytes();
        }
        if let Err(error) = self.signal.lock().recover_poison().write_all(&packet) {
            log::debug!("could not resize the handed-off Windows console: {error}");
        }
    }

    fn set_console_palette(&self, _palette: ConsolePalette) {}
}

/// The four console handles every version of `EstablishPtyHandoff` passes,
/// borrowed for the duration of the call: the console's signal pipe, its
/// reference handle, the server connection and the client process. Each is
/// duplicated before the call returns; see `WindowsHandoffRequest`.
#[derive(Clone, Copy)]
struct ConsoleHandles {
    signal: HANDLE,
    reference: HANDLE,
    server: HANDLE,
    client: HANDLE,
}

#[implement(ITerminalHandoff, ITerminalHandoff2, ITerminalHandoff3)]
struct TerminalHandoff {
    commands: UnboundedSender<crate::process_control::ProcessControlCommand>,
}

impl TerminalHandoff {
    fn submit(&self, request: WindowsHandoffRequest) -> bool {
        let (completion, completed) = channel();
        if self
            .commands
            .unbounded_send(
                crate::process_control::ProcessControlCommand::OpenWindowsHandoff {
                    request,
                    completion,
                },
            )
            .is_err()
        {
            return false;
        }
        completed
            .recv_timeout(Duration::from_secs(10))
            .is_ok_and(|accepted| accepted)
    }

    fn establish_legacy(
        &self,
        input: HANDLE,
        output: HANDLE,
        handles: ConsoleHandles,
        startup: Option<&TerminalStartupInfo>,
    ) -> HRESULT {
        // SAFETY: GetProcessId only reads a handle value; an invalid one is
        // reported as 0, which is refused below.
        let child_pid = unsafe { GetProcessId(handles.client) };
        if child_pid == 0 {
            return E_INVALIDARG;
        }
        let Ok(request) =
            duplicate_legacy_handoff_request(input, output, handles, child_pid, startup)
        else {
            return E_FAIL;
        };
        if self.submit(request) { S_OK } else { E_FAIL }
    }

    fn establish_v3(
        &self,
        input: *mut HANDLE,
        output: *mut HANDLE,
        handles: ConsoleHandles,
        startup: &TerminalStartupInfo,
    ) -> HRESULT {
        // SAFETY: GetProcessId only reads a handle value; an invalid one is
        // reported as 0, which is refused below.
        let child_pid = unsafe { GetProcessId(handles.client) };
        if child_pid == 0 {
            return E_INVALIDARG;
        }
        let Ok((server_pipe, client_pipe)) = create_handoff_pipe() else {
            return E_FAIL;
        };
        let Ok(conin) = duplicate_handle(HANDLE(server_pipe.as_raw_handle())) else {
            return E_FAIL;
        };
        let Ok(returned_output) = duplicate_handle(HANDLE(client_pipe.as_raw_handle())) else {
            return E_FAIL;
        };
        let Ok(request) =
            duplicate_handoff_request(server_pipe, conin, handles, child_pid, Some(startup))
        else {
            return E_FAIL;
        };
        if !self.submit(request) {
            return E_FAIL;
        }

        // ITerminalHandoff3 transfers ownership of these handles to the COM
        // caller. Do this only after the application accepted the request so
        // failed handoffs do not leak handles into the caller's process.
        // SAFETY: the only caller, ITerminalHandoff3::EstablishPtyHandoff,
        // checked both out-pointers non-null and COM gives them writable. The
        // signature does not enforce this; it should be an `unsafe fn`.
        unsafe {
            *input = HANDLE(client_pipe.into_raw_handle());
            *output = HANDLE(returned_output.into_raw_handle());
        }
        S_OK
    }
}

impl ITerminalHandoff_Impl for TerminalHandoff_Impl {
    unsafe fn EstablishPtyHandoff(
        &self,
        input: HANDLE,
        output: HANDLE,
        signal: HANDLE,
        reference: HANDLE,
        server: HANDLE,
        client: HANDLE,
    ) -> HRESULT {
        let handles = ConsoleHandles {
            signal,
            reference,
            server,
            client,
        };
        self.establish_legacy(input, output, handles, None)
    }
}

impl ITerminalHandoff2_Impl for TerminalHandoff_Impl {
    unsafe fn EstablishPtyHandoff(
        &self,
        input: HANDLE,
        output: HANDLE,
        signal: HANDLE,
        reference: HANDLE,
        server: HANDLE,
        client: HANDLE,
        startup: TerminalStartupInfo,
    ) -> HRESULT {
        let handles = ConsoleHandles {
            signal,
            reference,
            server,
            client,
        };
        self.establish_legacy(input, output, handles, Some(&startup))
    }
}

impl ITerminalHandoff3_Impl for TerminalHandoff_Impl {
    unsafe fn EstablishPtyHandoff(
        &self,
        input: *mut HANDLE,
        output: *mut HANDLE,
        signal: HANDLE,
        reference: HANDLE,
        server: HANDLE,
        client: HANDLE,
        startup: *const TerminalStartupInfo,
    ) -> HRESULT {
        if input.is_null() || output.is_null() || startup.is_null() {
            return E_INVALIDARG;
        }
        let handles = ConsoleHandles {
            signal,
            reference,
            server,
            client,
        };
        // SAFETY: `startup` was checked non-null above, and the
        // ITerminalHandoff3 contract passes a TerminalStartupInfo that stays
        // valid until this method returns, which outlives the borrow.
        let startup = unsafe { &*startup };
        self.establish_v3(input, output, handles, startup)
    }
}

#[implement(IClassFactory)]
struct TerminalHandoffFactory {
    commands: UnboundedSender<crate::process_control::ProcessControlCommand>,
}

impl IClassFactory_Impl for TerminalHandoffFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Ref<windows_core::IUnknown>,
        riid: *const windows_core::GUID,
        ppvobject: *mut *mut c_void,
    ) -> windows_core::Result<()> {
        if ppvobject.is_null() || riid.is_null() {
            return Err(windows_core::Error::from(E_INVALIDARG));
        }
        // SAFETY: COM passes CreateInstance a writable interface
        // out-pointer, checked non-null above.
        unsafe {
            *ppvobject = std::ptr::null_mut();
        }
        if punkouter.is_some() {
            return Err(windows_core::Error::from(E_INVALIDARG));
        }
        let object: ITerminalHandoff3 = TerminalHandoff {
            commands: self.commands.clone(),
        }
        .into();
        // SAFETY: `riid` and `ppvobject` are the caller's non-null IID and
        // out-pointer, checked above; `query` writes an AddRef'd interface or
        // null through `ppvobject`.
        unsafe { object.query(riid, ppvobject).ok() }
    }

    fn LockServer(&self, _: windows_core::BOOL) -> windows_core::Result<()> {
        Ok(())
    }
}

fn duplicate_legacy_handoff_request(
    input: HANDLE,
    output: HANDLE,
    handles: ConsoleHandles,
    child_pid: u32,
    startup: Option<&TerminalStartupInfo>,
) -> windows_core::Result<WindowsHandoffRequest> {
    // `input` is the handle the terminal reads from and `output` is the
    // handle it writes to. The names are from the COM contract, not from
    // Zetta's `conin`/`conout` fields.
    duplicate_handoff_request(
        duplicate_handle(input)?,
        duplicate_handle(output)?,
        handles,
        child_pid,
        startup,
    )
}

fn duplicate_handoff_request(
    conout: OwnedHandle,
    conin: OwnedHandle,
    handles: ConsoleHandles,
    child_pid: u32,
    startup: Option<&TerminalStartupInfo>,
) -> windows_core::Result<WindowsHandoffRequest> {
    Ok(WindowsHandoffRequest {
        conin,
        conout,
        signal: duplicate_handle(handles.signal)?,
        reference: duplicate_handle(handles.reference)?,
        server: duplicate_handle(handles.server)?,
        client: duplicate_handle(handles.client)?,
        child_pid,
        startup: startup.map(startup_metadata),
    })
}

/// Creates the duplex endpoint required by ITerminalHandoff3. The server side
/// stays in Zetta; the overlapped client side is returned to OpenConsole and
/// is passed to ConPTY as both its input and output handle.
///
/// Both ends are opened here and travel as handles, so the name is never
/// published and nothing needs to find the pipe by it. That makes everything
/// about the name a matter of keeping other accounts out: the pipe namespace
/// is machine-wide, a pipe's DACL is its first creator's, and the default DACL
/// lets other accounts read. So the name is random, the first (and only)
/// instance refuses a name somebody already holds, the DACL admits only this
/// account and SYSTEM, and the client end will not let the server impersonate
/// it beyond identification should it ever be connected to the wrong server.
fn create_handoff_pipe() -> windows_core::Result<(OwnedHandle, OwnedHandle)> {
    create_handoff_pipe_named(&handoff_pipe_name()?)
}

fn create_handoff_pipe_named(name: &str) -> windows_core::Result<(OwnedHandle, OwnedHandle)> {
    let name = HSTRING::from(name);
    let pipe_mode = NAMED_PIPE_MODE(
        PIPE_TYPE_BYTE.0 | PIPE_READMODE_BYTE.0 | PIPE_WAIT.0 | PIPE_REJECT_REMOTE_CLIENTS.0,
    );
    let security = crate::private_fs::UserOnlySecurity::new(
        crate::private_fs::UserOnlySecurityTarget::Object,
    )?;
    // SAFETY: `name` is a NUL-terminated HSTRING and `security` keeps the
    // SECURITY_ATTRIBUTES and its descriptor alive across the call.
    let server = unsafe {
        CreateNamedPipeW(
            &name,
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            pipe_mode,
            1,
            128 * 1024,
            128 * 1024,
            0,
            Some(std::ptr::from_ref(security.attributes())),
        )
    };
    if server.is_invalid() {
        return Err(windows_core::Error::from_win32());
    }
    // SAFETY: CreateNamedPipeW returned a valid, newly owned handle.
    let server = unsafe { OwnedHandle::from_raw_handle(server.0) };
    // SAFETY: `name` is a NUL-terminated HSTRING that outlives the call, and
    // no security attributes or template handle are passed.
    let client = unsafe {
        CreateFileW(
            &name,
            FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
            FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0),
            None,
            OPEN_EXISTING,
            handoff_client_flags(),
            None,
        )?
    };
    // SAFETY: CreateFileW returned a valid, newly owned handle.
    let client = unsafe { OwnedHandle::from_raw_handle(client.0) };
    Ok((server, client))
}

/// A pipe name nobody can predict: 128 random bits.
fn handoff_pipe_name() -> windows_core::Result<String> {
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| windows_core::Error::from(E_FAIL))?;
    Ok(format!(
        r"\\.\pipe\ZettaTerminalHandoff-{}",
        nonce
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

/// The client end's open flags: overlapped for ConPTY, and identification-only
/// impersonation for whichever server it reaches.
fn handoff_client_flags() -> FILE_FLAGS_AND_ATTRIBUTES {
    FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION
}

fn duplicate_handle(handle: HANDLE) -> windows_core::Result<OwnedHandle> {
    // Refused rather than duplicated: -1 is also this process's own
    // pseudo-handle.
    if handle.is_invalid() {
        return Err(windows_core::Error::from(E_INVALIDARG));
    }
    // SAFETY: the handoff contract passes handles that stay open until the
    // COM call returns, and this borrow ends inside it.
    let borrowed = unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(handle.0) };
    Ok(borrowed.try_clone_to_owned()?)
}

fn startup_metadata(info: &TerminalStartupInfo) -> HandoffStartupMetadata {
    HandoffStartupMetadata {
        title: bstr_to_string(&info.pszTitle),
    }
}

/// `None` for a missing or empty string. A BSTR knows its own length, so this
/// reads exactly that many characters rather than scanning for a NUL.
fn bstr_to_string(value: &windows_core::BSTR) -> Option<String> {
    (!value.is_empty()).then(|| String::from_utf16_lossy(value))
}

/// Starts the out-of-process COM class object used by Windows console
/// delegation. The thread stays alive for the lifetime of the embedding
/// process; GPUI owns the application lifetime and shuts down the process when
/// the last window/session is gone.
pub(crate) fn start_handoff_server(
    commands: UnboundedSender<crate::process_control::ProcessControlCommand>,
) -> Result<()> {
    thread::Builder::new()
        .name("zetta-terminal-handoff".to_owned())
        .spawn(move || {
            // SAFETY: CoInitializeEx takes no pointers here; this new thread
            // joins the MTA for the rest of its (never-ending) life.
            let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            if initialized.is_err() {
                eprintln!(
                    "Could not initialize the Windows terminal handoff COM server: {initialized}"
                );
                return;
            }
            let factory: IClassFactory = TerminalHandoffFactory { commands }.into();
            // SAFETY: COM is initialized on this thread; the CLSID is static
            // and `factory` is a live IClassFactory COM takes a reference to.
            let cookie = unsafe {
                CoRegisterClassObject(
                    &ZETTA_TERMINAL_HANDOFF_CLSID,
                    &factory,
                    CLSCTX_LOCAL_SERVER,
                    REGCLS_MULTIPLEUSE,
                )
            };
            if let Err(error) = cookie {
                eprintln!("Could not register the Windows terminal handoff COM server: {error}");
                // SAFETY: balances the successful CoInitializeEx above, on
                // the same thread, and nothing registered survives it.
                unsafe { CoUninitialize() };
                return;
            }
            loop {
                thread::park();
            }
        })
        .context("starting the Windows terminal handoff COM server")?;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/windows_integration/handoff.rs"]
mod tests;
