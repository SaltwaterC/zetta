//! No-`zmux` application shims.
//!
//! The terminal-spawn path is shared by both builds. In this build these
//! values deliberately do nothing: a missing provider makes the terminal use
//! its normal local PTY, and the pane/session registry has no daemon-owned
//! identifiers to track.

use std::sync::Arc;

use anyhow::Result;

pub(crate) struct MuxPtyProvider;

impl terminal::PtyProvider for MuxPtyProvider {
    fn open(&self, _: terminal::PtySpawnRequest) -> Result<terminal::PtyHandover> {
        anyhow::bail!("session multiplexer support is disabled in this build")
    }
}

#[derive(Default)]
pub(crate) struct MuxPanes;

impl MuxPanes {
    pub(crate) fn is_remote_tab(&self, _: u64) -> bool {
        false
    }

    pub(crate) fn session_id(&self, _: u64) -> Option<u64> {
        None
    }

    pub(crate) fn holds_session(&self, _: u64) -> bool {
        false
    }

    pub(crate) fn forget_pane(&mut self, _: u64) {}

    pub(crate) fn forget_tab(&mut self, _: u64) {}
}

impl crate::Zetta {
    pub(crate) fn has_shared_tab_binding(&self, _: u64) -> bool {
        false
    }

    pub(crate) fn mux_provider_for_tab(
        &mut self,
        _: u64,
        _: &mut gpui::Context<Self>,
    ) -> Result<Option<Arc<MuxPtyProvider>>> {
        Ok(None)
    }

    pub(crate) fn mux_provider_for_tab_with_restore_replay(
        &mut self,
        _: u64,
        _: Option<Vec<u8>>,
        _: &mut gpui::Context<Self>,
    ) -> Result<Option<Arc<MuxPtyProvider>>> {
        Ok(None)
    }

    pub(crate) fn adopt_mux_pane(
        &mut self,
        _: u64,
        _: u64,
        _: Option<&MuxPtyProvider>,
        _: &mut terminal::TerminalBuilder,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) {
    }

    pub(crate) fn release_mux_pane(&mut self, _: u64, _: u64, _: &mut gpui::App) {}

    pub(crate) fn request_shared_pane_close(
        &mut self,
        _: u64,
        _: u64,
        _: &mut gpui::Context<crate::Zetta>,
    ) -> bool {
        false
    }

    pub(crate) fn forget_shared_pane_mapping(&mut self, _: u64, _: u64) {}

    pub(crate) fn shared_pane_is_closing(&self, _: u64) -> bool {
        false
    }

    pub(crate) fn leave_shared_tab(&mut self, _: u64, _: &mut gpui::App) {}

    pub(crate) fn sync_shared_tab_state(&mut self, _: u64, _: &mut gpui::Context<crate::Zetta>) {}
}
