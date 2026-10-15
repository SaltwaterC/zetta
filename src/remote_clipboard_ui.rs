//! Per-viewer permissions for a tab's panes to read the host clipboard (paste,
//! off until granted) and to write it (copy, through zcopy or OSC 52, on until
//! withdrawn). Withdrawing either ends the transfers of that kind in flight.

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
        self.update_tab_terminals(tab_id, cx, |terminal| {
            terminal.set_remote_clipboard_paste_allowed(allowed);
        });
    }

    pub(crate) fn toggle_remote_clipboard_copy(
        &mut self,
        _: &ToggleRemoteClipboardCopy,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_id) = self.tabs.get(self.active_tab).map(|tab| tab.id) else {
            return;
        };
        let allowed = if self.remote_clipboard_copy_blocked_tabs.insert(tab_id) {
            false
        } else {
            self.remote_clipboard_copy_blocked_tabs.remove(&tab_id);
            true
        };
        self.update_tab_terminals(tab_id, cx, |terminal| {
            terminal.set_remote_clipboard_copy_allowed(allowed);
        });
        cx.notify();
    }

    fn update_tab_terminals(
        &self,
        tab_id: u64,
        cx: &mut Context<Self>,
        mut apply: impl FnMut(&mut Terminal),
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
            terminal.update(cx, |terminal, _| apply(terminal));
        }
    }
}
