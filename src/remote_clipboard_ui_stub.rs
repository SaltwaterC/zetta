//! Disabled-build surface for the remote clipboard tab action.

use super::*;

impl Zetta {
    pub(crate) fn toggle_remote_clipboard_paste(
        &mut self,
        _: &ToggleRemoteClipboardPaste,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }

    pub(crate) fn set_tab_remote_clipboard_paste(&self, _: u64, _: bool, _: &mut Context<Self>) {}
}
