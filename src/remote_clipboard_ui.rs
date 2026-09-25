//! Per-viewer permission for reading the host clipboard from a remote pane.

use super::*;

impl Zetta {
    pub(crate) fn toggle_remote_clipboard_paste(
        &mut self,
        _: &ToggleRemoteClipboardPaste,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_id) = self.tabs.get(self.active_tab).map(|tab| tab.id) else {
            return;
        };
        let allowed = if self.remote_clipboard_paste_tabs.insert(tab_id) {
            true
        } else {
            self.remote_clipboard_paste_tabs.remove(&tab_id);
            false
        };
        self.set_tab_remote_clipboard_paste(tab_id, allowed, cx);
        cx.notify();
    }

    pub(crate) fn set_tab_remote_clipboard_paste(
        &self,
        tab_id: u64,
        allowed: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == tab_id) else {
            return;
        };
        let terminals = tab
            .panes
            .iter()
            .flat_map(TerminalPane::all_terminals)
            .cloned()
            .collect::<Vec<_>>();
        for terminal in terminals {
            terminal.update(cx, |terminal, _| {
                terminal.set_remote_clipboard_paste_allowed(allowed);
            });
        }
    }
}
