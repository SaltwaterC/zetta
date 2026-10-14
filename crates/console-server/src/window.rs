// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! The window GetConsoleWindow returns. A pseudoconsole has no visible
//! window, but programs expect a real HWND: to parent dialogs, to test for a
//! console, or to call ShowWindow on it. This is InteractivityFactory's
//! "PseudoConsoleWindow": hidden, never activated, pumped on its own thread,
//! and reporting ShowWindow calls as XTWINOPS iconify/de-iconify.
use std::io;
use std::sync::{Arc, OnceLock, mpsc};
use std::thread::JoinHandle;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// Called with true to show and false to hide.
pub type Visibility = Arc<dyn Fn(bool) + Send + Sync>;

pub struct PseudoWindow {
    hwnd: usize,
    thread: Option<JoinHandle<()>>,
}

fn class_name() -> &'static [u16] {
    static NAME: OnceLock<Vec<u16>> = OnceLock::new();
    NAME.get_or_init(|| "PseudoConsoleWindow".encode_utf16().chain([0]).collect())
}

/// Registered once per process; every console's window shares the class.
fn register_class() -> io::Result<()> {
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    let registered = *REGISTERED.get_or_init(|| {
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: unsafe { GetModuleHandleW(std::ptr::null()) },
            lpszClassName: class_name().as_ptr(),
            ..unsafe { std::mem::zeroed() }
        };
        unsafe { RegisterClassExW(&class) != 0 }
    });
    if registered {
        Ok(())
    } else {
        Err(io::Error::other(
            "RegisterClassExW(PseudoConsoleWindow) failed",
        ))
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let visibility = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Visibility;
    let report = |shown: bool| {
        if !visibility.is_null() {
            unsafe { (*visibility)(shown) };
        }
    };
    match message {
        WM_SIZE => {
            match wparam as u32 {
                SIZE_RESTORED => report(true),
                SIZE_MINIMIZED => report(false),
                _ => {}
            }
            0
        }
        // lParam 0: somebody called ShowWindow on us.
        WM_SHOWWINDOW => {
            if lparam == 0 {
                report(wparam != 0);
            }
            0
        }
        WM_DESTROY => {
            if !visibility.is_null() {
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(visibility as *mut Visibility));
                    PostQuitMessage(0);
                }
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

impl PseudoWindow {
    /// Creates the window on a thread of its own, which pumps its messages
    /// until the window is dropped.
    pub fn create(owner: Option<usize>, visibility: Visibility) -> io::Result<Self> {
        register_class()?;
        let (sender, receiver) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("console-window".into())
            .spawn(move || {
                let hwnd = unsafe {
                    CreateWindowExW(
                        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
                        class_name().as_ptr(),
                        std::ptr::null(),
                        WS_POPUP,
                        0,
                        0,
                        0,
                        0,
                        owner.unwrap_or(0) as HWND,
                        std::ptr::null_mut(),
                        GetModuleHandleW(std::ptr::null()),
                        std::ptr::null(),
                    )
                };
                if hwnd.is_null() {
                    let _ = sender.send(Err(io::Error::last_os_error()));
                    return;
                }
                let boxed = Box::into_raw(Box::new(visibility));
                unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, boxed as isize) };
                let _ = sender.send(Ok(hwnd as usize));
                let mut message: MSG = unsafe { std::mem::zeroed() };
                while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
                    unsafe {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
            })?;
        let hwnd = receiver
            .recv()
            .map_err(|_| io::Error::other("console window thread ended"))??;
        Ok(Self {
            hwnd,
            thread: Some(thread),
        })
    }
    pub fn hwnd(&self) -> usize {
        self.hwnd
    }
}

impl Drop for PseudoWindow {
    fn drop(&mut self) {
        // WM_CLOSE reaches DefWindowProc, which destroys the window on its
        // own thread; WM_DESTROY then ends the message loop.
        unsafe {
            PostMessageW(self.hwnd as HWND, WM_CLOSE, 0, 0);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
#[path = "tests/window.rs"]
mod tests;
