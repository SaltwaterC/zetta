use log::{info, warn};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::io::{Error, Result};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, IntoRawHandle, OwnedHandle};
use std::{mem, ptr, time::Duration};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, S_OK, WAIT_OBJECT_0};
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::core::{HRESULT, PWSTR};
use windows_sys::{s, w};

use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateEventW, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcessId, GetExitCodeProcess,
    InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION,
    STARTF_USEFILLATTRIBUTE, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

use crate::event::{OnResize, WindowSize};
use crate::tty::windows::child::ChildExitWatcher;
use crate::tty::windows::{cmdline, normalize_working_directory, win32_string};
use crate::tty::{ConsolePalette, Options};

/// Visible to the parent module so an *attached* console is unblocked through
/// the same buffer as one created here, by construction rather than by a
/// second constant that can drift from this one.
pub(super) const PIPE_CAPACITY: usize = crate::event_loop::READ_BUFFER_SIZE;

/// Load the pseudoconsole API from conpty.dll if possible, otherwise use the
/// standard Windows API.
///
/// The conpty.dll from the Windows Terminal project
/// supports loading OpenConsole.exe, which offers many improvements and
/// bugfixes compared to the standard conpty that ships with Windows.
///
/// The conpty.dll and OpenConsole.exe files will be searched in PATH and in
/// the directory where Alacritty's executable is located.
type CreatePseudoConsoleFn =
    unsafe extern "system" fn(COORD, HANDLE, HANDLE, u32, *mut HPCON) -> HRESULT;
type ResizePseudoConsoleFn = unsafe extern "system" fn(HPCON, COORD) -> HRESULT;
type ClosePseudoConsoleFn = unsafe extern "system" fn(HPCON);

struct ConptyApi {
    create: CreatePseudoConsoleFn,
    resize: ResizePseudoConsoleFn,
    close: ClosePseudoConsoleFn,
}

impl ConptyApi {
    fn new() -> Self {
        match Self::load_conpty() {
            Some(conpty) => {
                info!("Using conpty.dll for pseudoconsole");
                conpty
            },
            None => {
                // Cannot load conpty.dll - use the standard Windows API.
                info!("Using Windows API for pseudoconsole");
                Self {
                    create: CreatePseudoConsole,
                    resize: ResizePseudoConsole,
                    close: ClosePseudoConsole,
                }
            },
        }
    }

    /// Try loading ConptyApi from conpty.dll library.
    fn load_conpty() -> Option<Self> {
        type LoadedFn = unsafe extern "system" fn() -> isize;
        unsafe {
            let hmodule = LoadLibraryW(w!("conpty.dll"));
            if hmodule.is_null() {
                return None;
            }
            let create_fn = GetProcAddress(hmodule, s!("CreatePseudoConsole"))?;
            let resize_fn = GetProcAddress(hmodule, s!("ResizePseudoConsole"))?;
            let close_fn = GetProcAddress(hmodule, s!("ClosePseudoConsole"))?;

            Some(Self {
                create: mem::transmute::<LoadedFn, CreatePseudoConsoleFn>(create_fn),
                resize: mem::transmute::<LoadedFn, ResizePseudoConsoleFn>(resize_fn),
                close: mem::transmute::<LoadedFn, ClosePseudoConsoleFn>(close_fn),
            })
        }
    }
}

/// RAII Pseudoconsole.
pub struct Conpty {
    pub handle: HPCON,
    api: ConptyApi,
    palette_helper: Option<std::path::PathBuf>,
}

impl Drop for Conpty {
    fn drop(&mut self) {
        // XXX: This will block until the conout pipe is drained. Will cause a deadlock if the
        // conout pipe has already been dropped by this point.
        //
        // See PR #3084 and https://docs.microsoft.com/en-us/windows/console/closepseudoconsole.
        unsafe { (self.api.close)(self.handle) }
    }
}

// The ConPTY handle can be sent between threads.
unsafe impl Send for Conpty {}

pub(super) struct SpawnedPty {
    pub(super) backend: Conpty,
    pub(super) conout: OwnedHandle,
    pub(super) conin: OwnedHandle,
    pub(super) child_watcher: ChildExitWatcher,
}

pub(super) fn new(config: &Options, window_size: WindowSize) -> Result<SpawnedPty> {
    let api = ConptyApi::new();
    let mut pty_handle: HPCON = 0;

    // Passing 0 as the size parameter allows the "system default" buffer
    // size to be used. There may be small performance and memory advantages
    // to be gained by tuning this in the future, but it's likely a reasonable
    // start point.
    let (conout, conout_pty_handle) = miow::pipe::anonymous(0)?;
    let (conin_pty_handle, conin) = miow::pipe::anonymous(0)?;

    // Create the Pseudo Console, using the pipes.
    let result = unsafe {
        (api.create)(
            window_size.into(),
            conin_pty_handle.into_raw_handle() as HANDLE,
            conout_pty_handle.into_raw_handle() as HANDLE,
            0,
            &mut pty_handle as *mut _,
        )
    };

    assert_eq!(result, S_OK);

    // Native children are launched by the bundled bootstrap. It performs the
    // loader-breakpoint palette synchronization after this function returns,
    // once the caller is already servicing ConPTY's terminal queries.
    let proc_info = match config.console_palette_helper.as_ref() {
        Some(helper) => spawn_palette_bootstrap(pty_handle, config, helper),
        None => spawn_attached_process(pty_handle, config),
    }?;

    // `ChildExitWatcher` takes ownership of the process handle. The thread
    // handle is not needed after CreateProcessW returns and must be closed
    // independently.
    let child_watcher = ChildExitWatcher::new(proc_info.hProcess);
    unsafe {
        CloseHandle(proc_info.hThread);
    }
    let child_watcher = child_watcher?;
    let conpty = Conpty {
        handle: pty_handle as HPCON,
        api,
        palette_helper: config.console_palette_helper.clone(),
    };

    // Keep the console pipes as raw handles until the caller decides whether
    // it is creating an ordinary terminal (which needs the unblocking reader
    // and writer) or a multiplexer host console (which must not read at all).
    // The latter hands duplicates to the attached client; putting the pipes in
    // `UnblockedReader` here would make the host compete with that client for
    // ConPTY output.
    let conout = unsafe { OwnedHandle::from_raw_handle(conout.into_raw_handle()) };
    let conin = unsafe { OwnedHandle::from_raw_handle(conin.into_raw_handle()) };

    Ok(SpawnedPty { backend: conpty, conout, conin, child_watcher })
}

const PALETTE_HELPER_ENV: &str = "ZETTA_INTERNAL_CONSOLE_PALETTE_V1";
const PALETTE_READY_EVENT_ENV: &str = "ZETTA_INTERNAL_CONSOLE_PALETTE_READY_EVENT_V1";
const PALETTE_BOOTSTRAP_ENV: &str = "ZETTA_INTERNAL_CONSOLE_PALETTE_BOOTSTRAP_V1";
const PALETTE_HELPER_TIMEOUT: Duration = Duration::from_secs(2);
static PALETTE_EVENT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn spawn_palette_bootstrap(
    pty_handle: HPCON,
    config: &Options,
    helper: &std::path::Path,
) -> Result<PROCESS_INFORMATION> {
    let mut bootstrap = config.clone();
    bootstrap.env.retain(|key, _| {
        !key.eq_ignore_ascii_case(PALETTE_HELPER_ENV)
            && !key.eq_ignore_ascii_case(PALETTE_READY_EVENT_ENV)
            && !key.eq_ignore_ascii_case(PALETTE_BOOTSTRAP_ENV)
    });
    bootstrap.shell =
        Some(crate::tty::Shell::new(helper.to_string_lossy().into_owned(), Vec::new()));
    bootstrap.escape_args = true;
    bootstrap
        .env
        .insert(PALETTE_HELPER_ENV.to_owned(), config.console_palette.to_private_payload());
    bootstrap.env.insert(PALETTE_BOOTSTRAP_ENV.to_owned(), cmdline(config));
    spawn_attached_process(pty_handle, &bootstrap)
}

fn start_palette_helper(
    pty_handle: HPCON,
    helper: &std::path::Path,
    palette: ConsolePalette,
) -> Result<PROCESS_INFORMATION> {
    let event_name = format!(
        "Local\\ZettaPalette-{}-{}",
        unsafe { GetCurrentProcessId() },
        PALETTE_EVENT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let event_name_wide = win32_string(OsStr::new(&event_name));
    let ready_event =
        unsafe { CreateEventW(ptr::null(), true as i32, false as i32, event_name_wide.as_ptr()) };
    if ready_event.is_null() {
        return Err(Error::last_os_error());
    }
    let mut env = HashMap::new();
    env.insert(PALETTE_HELPER_ENV.to_owned(), palette.to_private_payload());
    env.insert(PALETTE_READY_EVENT_ENV.to_owned(), event_name);
    let helper_config = Options {
        shell: Some(crate::tty::Shell::new(helper.to_string_lossy().into_owned(), Vec::new())),
        env,
        escape_args: true,
        ..Options::default()
    };
    let process = match spawn_attached_process(pty_handle, &helper_config) {
        Ok(process) => process,
        Err(error) => {
            unsafe { CloseHandle(ready_event) };
            return Err(error);
        },
    };
    let timeout = u32::try_from(PALETTE_HELPER_TIMEOUT.as_millis()).unwrap_or(u32::MAX);
    let wait = unsafe { WaitForSingleObject(ready_event, timeout) };
    unsafe { CloseHandle(ready_event) };
    if wait != WAIT_OBJECT_0 {
        terminate_palette_helper(process);
        return Err(Error::other("palette helper timed out"));
    }
    Ok(process)
}

fn finish_palette_helper(process: PROCESS_INFORMATION) -> Result<()> {
    let timeout = u32::try_from(PALETTE_HELPER_TIMEOUT.as_millis()).unwrap_or(u32::MAX);
    if unsafe { WaitForSingleObject(process.hProcess, timeout) } != WAIT_OBJECT_0 {
        terminate_palette_helper(process);
        return Err(Error::other("palette helper did not exit"));
    }
    let mut exit_code = 1;
    let got_exit = unsafe { GetExitCodeProcess(process.hProcess, &mut exit_code) } > 0;
    unsafe {
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    if !got_exit {
        return Err(Error::last_os_error());
    }
    if exit_code != 0 {
        return Err(Error::other(format!("palette helper exited with status {exit_code}")));
    }
    Ok(())
}

fn terminate_palette_helper(process: PROCESS_INFORMATION) {
    terminate_process(process);
}

fn terminate_process(process: PROCESS_INFORMATION) {
    unsafe {
        let _ = TerminateProcess(process.hProcess, 1);
        WaitForSingleObject(process.hProcess, 1000);
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
}

fn spawn_attached_process(pty_handle: HPCON, config: &Options) -> Result<PROCESS_INFORMATION> {
    let mut size = 0;
    let mut startup_info_ex: STARTUPINFOEXW = unsafe { mem::zeroed() };
    startup_info_ex.StartupInfo.lpTitle = ptr::null_mut() as PWSTR;
    startup_info_ex.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
    startup_info_ex.StartupInfo.dwFlags |= STARTF_USESTDHANDLES;
    if config.console_palette_helper.is_some() {
        // ConPTY remembers this separately from the current buffer
        // attributes. SGR 39/49 restore it, so seed it before the console is
        // initialized in addition to applying the full RGB table below.
        startup_info_ex.StartupInfo.dwFlags |= STARTF_USEFILLATTRIBUTE;
        startup_info_ex.StartupInfo.dwFillAttribute =
            u32::from(config.console_palette.win32_attributes());
    }

    unsafe {
        if InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size) > 0 {
            return Err(Error::last_os_error());
        }
    }
    let mut attr_list: Box<[u8]> = vec![0; size].into_boxed_slice();
    #[allow(clippy::cast_ptr_alignment)]
    {
        startup_info_ex.lpAttributeList = attr_list.as_mut_ptr() as _;
    }
    unsafe {
        if InitializeProcThreadAttributeList(startup_info_ex.lpAttributeList, 1, 0, &mut size) == 0
        {
            return Err(Error::last_os_error());
        }
        if UpdateProcThreadAttribute(
            startup_info_ex.lpAttributeList,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            pty_handle as *mut std::ffi::c_void,
            mem::size_of::<HPCON>(),
            ptr::null_mut(),
            ptr::null_mut(),
        ) == 0
        {
            DeleteProcThreadAttributeList(startup_info_ex.lpAttributeList);
            return Err(Error::last_os_error());
        }
    }

    let mut command_line = win32_string(&cmdline(config));
    let cwd = config.working_directory.as_deref().map(|directory| {
        let directory = normalize_working_directory(directory);
        win32_string(directory.as_os_str())
    });
    let mut creation_flags = EXTENDED_STARTUPINFO_PRESENT;
    let custom_env_block = convert_custom_env(&config.env);
    let custom_env_block_pointer = custom_env_block.as_ref().map_or(ptr::null_mut(), |block| {
        creation_flags |= CREATE_UNICODE_ENVIRONMENT;
        block.as_ptr() as *mut std::ffi::c_void
    });
    let mut process: PROCESS_INFORMATION = unsafe { mem::zeroed() };
    let success = unsafe {
        CreateProcessW(
            ptr::null(),
            command_line.as_mut_ptr() as PWSTR,
            ptr::null_mut(),
            ptr::null_mut(),
            false as i32,
            creation_flags,
            custom_env_block_pointer,
            cwd.as_ref().map_or_else(ptr::null, |value| value.as_ptr()),
            &startup_info_ex.StartupInfo,
            &mut process,
        ) > 0
    };
    unsafe { DeleteProcThreadAttributeList(startup_info_ex.lpAttributeList) };
    if !success {
        return Err(Error::last_os_error());
    }
    Ok(process)
}

// Windows environment variables are case-insensitive, and the caller is responsible for
// deduplicating environment variables, so do that here while converting.
//
// https://learn.microsoft.com/en-us/previous-versions/troubleshoot/windows/win32/createprocess-cannot-eliminate-duplicate-variables#environment-variables
fn convert_custom_env(custom_env: &HashMap<String, String>) -> Option<Vec<u16>> {
    // Windows inherits parent's env when no `lpEnvironment` parameter is specified.
    if custom_env.is_empty() {
        return None;
    }

    let mut converted_block = Vec::new();
    let mut all_env_keys = HashSet::new();
    for (custom_key, custom_value) in custom_env {
        let custom_key_os = OsStr::new(custom_key);
        if all_env_keys.insert(custom_key_os.to_ascii_uppercase()) {
            add_windows_env_key_value_to_block(
                &mut converted_block,
                custom_key_os,
                OsStr::new(&custom_value),
            );
        } else {
            warn!(
                "Omitting environment variable pair with duplicate key: \
                 '{custom_key}={custom_value}'"
            );
        }
    }

    // Pull the current process environment after, to avoid overwriting the user provided one.
    for (inherited_key, inherited_value) in std::env::vars_os() {
        if all_env_keys.insert(inherited_key.to_ascii_uppercase()) {
            add_windows_env_key_value_to_block(
                &mut converted_block,
                &inherited_key,
                &inherited_value,
            );
        }
    }

    converted_block.push(0);
    Some(converted_block)
}

// According to the `lpEnvironment` parameter description:
// https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessa#parameters
//
// > An environment block consists of a null-terminated block of null-terminated strings. Each
// string is in the following form:
// >
// > name=value\0
fn add_windows_env_key_value_to_block(block: &mut Vec<u16>, key: &OsStr, value: &OsStr) {
    block.extend(key.encode_wide());
    block.push('=' as u16);
    block.extend(value.encode_wide());
    block.push(0);
}

impl OnResize for Conpty {
    fn on_resize(&mut self, window_size: WindowSize) {
        let result = unsafe { (self.api.resize)(self.handle, window_size.into()) };
        assert_eq!(result, S_OK);
    }
}

impl Conpty {
    pub(super) fn set_console_palette(&self, palette: ConsolePalette) {
        let Some(helper) = &self.palette_helper else {
            return;
        };
        if let Err(error) =
            start_palette_helper(self.handle, helper, palette).and_then(finish_palette_helper)
        {
            warn!("could not update pseudoconsole colors: {error}");
        }
    }
}

impl From<WindowSize> for COORD {
    fn from(window_size: WindowSize) -> Self {
        let lines = window_size.num_lines;
        let columns = window_size.num_cols;
        COORD { X: columns as i16, Y: lines as i16 }
    }
}

/// Duplicates a console's pipes so they can be handed to another process later.
///
/// Returns `None` when either duplicate fails; a partial pair is useless, and
/// a terminal that simply cannot be handed over is better than one that hands
/// over half of itself.
pub(super) fn duplicate_console_pipes(
    conout: &impl std::os::windows::io::AsRawHandle,
    conin: &impl std::os::windows::io::AsRawHandle,
) -> Option<(std::os::windows::io::OwnedHandle, std::os::windows::io::OwnedHandle)> {
    Some((duplicate_handle(conout)?, duplicate_handle(conin)?))
}

fn duplicate_handle(
    handle: &impl std::os::windows::io::AsRawHandle,
) -> Option<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::BorrowedHandle;
    // SAFETY: the borrow lives only for this call, and the caller owns the
    // handle for at least that long.
    let borrowed = unsafe { BorrowedHandle::borrow_raw(handle.as_raw_handle()) };
    borrowed.try_clone_to_owned().ok()
}
