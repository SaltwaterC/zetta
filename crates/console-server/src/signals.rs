// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! Console control events an application raises itself, the console
//! window, and the end of a console whose clients will not leave.
use super::*;
use crate::window::PseudoWindow;

impl Server {
    /// CONSOLE_CTRLEVENT_MSG: CtrlEvent, ProcessGroupId. GenerateConsoleCtrlEvent
    /// reaches every client in the group (all of them for group 0), newest
    /// first, as ProcessCtrlEvents dispatches.
    pub(super) fn generate_ctrl_event(&mut self) -> (i32, usize) {
        let event = u32_at(&self.reply, 0);
        let group = u32_at(&self.reply, 4);
        if event > 1 {
            return (INVALID_PARAMETER, 0);
        }
        let targets: Vec<u32> = self
            .processes
            .values()
            .rev()
            .filter(|client| group == 0 || client.group == group || client.pid == group)
            .map(|client| client.pid)
            .collect();
        if group != 0 && targets.is_empty() {
            return (INVALID_PARAMETER, 0);
        }
        let window = self.window.as_ref().map_or(0, PseudoWindow::hwnd);
        for pid in targets {
            let _ = crate::control::send_ctrl_event(pid, event, window);
        }
        (0, 0)
    }

    /// GetConsoleWindow: the hidden pseudo window, made on first request.
    /// ShowWindow on it reaches the terminal as XTWINOPS 1/2.
    pub(super) fn get_console_window(&mut self) -> (i32, usize) {
        if self.window.is_none() {
            let output = self.window_output.clone();
            let visibility: crate::window::Visibility = std::sync::Arc::new(move |shown| {
                if let Some(output) = &output {
                    let mut output = output
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let _ = output.write(
                        if shown { b"\x1b[1t" } else { b"\x1b[2t" },
                        crate::output::OutputMode {
                            lf_implies_cr: false,
                            wrap: true,
                        },
                    );
                }
            });
            match PseudoWindow::create(self.window_owner, visibility) {
                Ok(window) => self.window = Some(window),
                Err(error) => eprintln!("console window: {error}"),
            }
        }
        self.reply.fill(0);
        let hwnd = self.window.as_ref().map_or(0, PseudoWindow::hwnd);
        self.reply[..size_of::<usize>()].copy_from_slice(&hwnd.to_ne_bytes());
        (0, 0)
    }

    /// The console's transport failed with clients still attached: end
    /// them, since they can no longer be served.
    pub(crate) fn terminate_clients(&self) {
        for client in self.processes.values() {
            crate::native::terminate_process(client.pid);
        }
    }
}
