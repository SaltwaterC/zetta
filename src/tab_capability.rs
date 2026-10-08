//! Which control-socket requests may act inside a tab.
//!
//! The endpoint token authenticates the channel, not the caller: every process
//! running as the user can read it. That is enough for an unprotected tab,
//! which any such process could attach anyway. A protected session is meant to
//! hold against other same-user code (`docs/background-sessions.md`), and a
//! window that typed whatever the socket sent into the active pane, or opened
//! panes beside it, would be a deputy straight across that boundary.
//!
//! Nothing a request can carry tells this window that it comes from inside the
//! tab. An inherited environment variable cannot: Yama restricts `ptrace`
//! attach, not the read access `/proc/<pid>/environ` needs, so any same-user
//! process can read a shell's environment. So a protected tab refuses these
//! requests outright, on every platform, and the `zetta pane` family works only
//! in unprotected tabs.
//!
//! Each entry point here checks before doing what the same-named method
//! without the `_from_control` suffix does, and is the only way
//! `startup/process_control_loop.rs` reaches a tab for it.

use anyhow::Result;
use gpui::{App, Context, Window};

use crate::{
    Zetta,
    command_panes::{PaneCommand, ShellCommandRequest},
    process_control::ReplacePaneRequest,
    run_command::{RunCommandRegistry, RunRegistration, RunWaitRequest},
};

impl Zetta {
    pub(crate) fn run_shell_command_from_control(
        &mut self,
        request: ShellCommandRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        self.refuse_if_active_tab_is_protected()?;
        self.run_shell_command(request, window, cx)
    }

    pub(crate) fn run_command_pane_from_control(
        &mut self,
        request: PaneCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        self.refuse_if_active_tab_is_protected()?;
        self.run_command_pane(request, window, cx)
    }

    pub(crate) fn replace_active_pane_from_control(
        &mut self,
        request: ReplacePaneRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.refuse_if_active_tab_is_protected().is_ok()
            && self.replace_active_pane_from_cli(request, window, cx)
    }

    pub(crate) fn register_run_wait_from_control(
        &self,
        request: RunWaitRequest,
        registry: &RunCommandRegistry,
        cx: &App,
    ) -> Result<RunRegistration> {
        self.refuse_if_tab_is_protected(request.owner.attention_id)?;
        self.register_run_wait(request, registry, cx)
    }

    pub(crate) fn command_pane_labels_from_control(
        &self,
        attention_id: Option<u64>,
    ) -> Result<Vec<String>> {
        match attention_id {
            Some(attention_id) => self.refuse_if_tab_is_protected(attention_id)?,
            None => self.refuse_if_active_tab_is_protected()?,
        }
        Ok(self.command_pane_labels_for_attention(attention_id))
    }

    fn refuse_if_tab_is_protected(&self, attention_id: u64) -> Result<()> {
        anyhow::ensure!(
            !self.tab_is_protected_by_attention_id(attention_id),
            "the tab is protected, so it does not take commands from outside its own window"
        );
        Ok(())
    }

    fn refuse_if_active_tab_is_protected(&self) -> Result<()> {
        match self.tabs.get(self.active_tab) {
            Some(tab) => self.refuse_if_tab_is_protected(tab.attention_id),
            None => Ok(()),
        }
    }

    fn tab_is_protected_by_attention_id(&self, attention_id: u64) -> bool {
        if let Some(tab) = self
            .tabs
            .iter()
            .find(|tab| tab.attention_id == attention_id)
        {
            return self.tab_is_protected(tab.id);
        }
        // Kept running in this process. Only the protected ones are missing
        // from the unprotected view, which is what decides it.
        self.background_sessions
            .iter()
            .any(|tab| tab.attention_id == attention_id)
            && !self
                .background_sessions
                .iter_unprotected()
                .any(|tab| tab.attention_id == attention_id)
    }
}

#[cfg(test)]
#[path = "tests/tab_capability.rs"]
mod tests;
