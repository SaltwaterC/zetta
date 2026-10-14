// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! Probe Microsoft's console control-event delivery for our own hosted child.
//! Success is determined by the child's handler marker, not the API status alone.
use std::ffi::c_void;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::{FreeLibrary, HANDLE};
use windows_sys::Win32::System::Console::{CTRL_C_EVENT, SetConsoleCtrlHandler};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

#[repr(C)]
struct EndTask {
    process_id: HANDLE,
    window: HANDLE,
    event: u32,
    flags: u32,
}
#[repr(C)]
struct ProcessInfo {
    pid: u32,
    flags: u32,
}
fn console_control<T>(command: u32, data: &mut T) -> io::Result<i32> {
    let name: Vec<u16> = "user32.dll".encode_utf16().chain([0]).collect();
    let raw = unsafe { LoadLibraryW(name.as_ptr()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let function = unsafe { GetProcAddress(raw, c"ConsoleControl".as_ptr().cast()) };
    let Some(function) = function else {
        unsafe {
            FreeLibrary(raw);
        }
        return Err(io::Error::other("user32!ConsoleControl unavailable"));
    };
    let function: unsafe extern "system" fn(u32, *mut c_void, u32) -> i32 =
        unsafe { std::mem::transmute(function) };
    let status = unsafe { function(command, (data as *mut T).cast(), size_of::<T>() as u32) };
    unsafe {
        FreeLibrary(raw);
    }
    Ok(status)
}
pub fn notify_application(pid: u32) -> io::Result<i32> {
    console_control(1, &mut ProcessInfo { pid, flags: 1 })
}
pub fn send_ctrl_c(pid: u32) -> io::Result<i32> {
    send_ctrl_event(pid, CTRL_C_EVENT, 0)
}
pub const CTRL_CLOSE_EVENT: u32 = 2;
/// Delivers a control event to one attached client, as OpenConsole's
/// ProcessCtrlEvents does through ConsoleControl(ConsoleEndTask): the flag
/// names the event, and `window` is the console's window, if it has one.
/// CSRSS gives a client five seconds to handle CTRL_CLOSE_EVENT.
pub fn send_ctrl_event(pid: u32, event: u32, window: usize) -> io::Result<i32> {
    let flags = match event {
        0 => 0x1,
        1 => 0x2,
        2 => 0x4,
        5 => 0x10,
        6 => 0x20,
        _ => 0,
    };
    console_control(
        7,
        &mut EndTask {
            process_id: pid as usize as HANDLE,
            window: window as HANDLE,
            event,
            flags,
        },
    )
}
static INTERRUPTED: AtomicBool = AtomicBool::new(false);
unsafe extern "system" fn handler(event: u32) -> i32 {
    if event == CTRL_C_EVENT {
        INTERRUPTED.store(true, Ordering::Release);
        1
    } else {
        0
    }
}
pub fn child_probe() -> io::Result<()> {
    // The launcher can inherit the ignore-Ctrl+C bit from its own host.
    // Registering a handler does not reset that inherited bit.
    if unsafe { SetConsoleCtrlHandler(None, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { SetConsoleCtrlHandler(Some(handler), 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    use std::io::Write;
    println!("ZETTA_CTRL_READY");
    io::stdout().flush()?;
    while !INTERRUPTED.load(Ordering::Acquire) {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    println!("ZETTA_CTRL_HANDLED");
    Ok(())
}
