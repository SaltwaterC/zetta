use super::*;

/// Shared by the HTTP and TFTP server panes: Ctrl-C stops the server instead
/// of being forwarded to it, since neither has a foreground process to
/// receive an interrupt.
pub(crate) fn ctrl_c_interrupts_byte_stream(input: &TerminalInput) -> bool {
    match input {
        TerminalInput::Keystroke(keystroke)
            if keystroke.key.eq_ignore_ascii_case("c")
                && keystroke.modifiers.control
                && !keystroke.modifiers.alt
                && !keystroke.modifiers.platform
                && !keystroke.modifiers.shift =>
        {
            true
        }
        TerminalInput::Text(text) => text.as_bytes() == [3],
        _ => false,
    }
}

#[cfg(byte_stream_panes)]
#[derive(Clone, Copy)]
#[allow(
    dead_code,
    reason = "each variant is built by a different, independently-featured caller"
)]
pub(crate) enum ByteStreamInputPolicy {
    /// Ctrl-C stops the byte stream (HTTP/TFTP servers).
    CloseOnInterrupt,
    /// Input is broadcast like any other terminal pane (serial console).
    Broadcast,
}

#[cfg(byte_stream_panes)]
pub(crate) struct ByteStreamPaneRequest {
    pub(crate) reader: Box<dyn std::io::Read + Send>,
    pub(crate) writer: Box<dyn std::io::Write + Send>,
    pub(crate) label: String,
    pub(crate) title: String,
    pub(crate) input: ByteStreamInputPolicy,
}

#[cfg(byte_stream_panes)]
impl Zetta {
    /// Opens a pane backed by a raw byte stream rather than a spawned process
    /// (an HTTP/TFTP server's log output, or a serial console connection).
    /// Returns the new pane's id, or `None` if the pane could not be opened.
    pub(crate) fn open_byte_stream_pane(
        &mut self,
        request: ByteStreamPaneRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let tab = self.tabs.get(self.active_tab)?;
        if !can_add_panes(tab.panes.len(), 1) {
            return None;
        }
        let tab_id = tab.id;
        let tab_theme_override = tab.theme_override.clone();
        let active_pane_id = tab.active_pane;
        let profile = tab.active_profile().cloned()?;
        let project = self.active_project_config().cloned();
        let terminal_theme = match resolve_terminal_theme(
            None,
            tab_theme_override.as_deref(),
            &profile,
            project.as_deref(),
            cx,
        ) {
            Ok(theme) => theme,
            Err(error) => {
                self.configuration_error =
                    Some(format!("Could not apply profile theme: {error:#}"));
                cx.notify();
                return None;
            }
        };
        let pane_id = self.next_pane_id;
        self.next_pane_id += 1;
        self.projects.inherit_pane_root(active_pane_id, pane_id);

        let tab = &mut self.tabs[self.active_tab];
        tab.maximized_pane = None;
        if !tab.layout.split(
            active_pane_id,
            SplitAxis::Vertical,
            pane_id,
            SplitPosition::After,
        ) {
            return None;
        }
        tab.push_pane(TerminalPane::new(pane_id, profile).with_generated_label(request.label));
        tab.activate_pane(pane_id);

        let settings = TerminalSpawnSettings::current(cx);
        let builder = TerminalBuilder::new_byte_stream(
            request.reader,
            request.writer,
            request.title,
            settings.cursor_shape,
            settings.alternate_scroll,
            settings.max_scroll_history_lines,
            cx.entity_id().as_u64(),
            cx.background_executor(),
            PathStyle::local(),
        );
        let terminal = cx.new(|cx| builder.subscribe(cx));
        let view = cx.new(|cx| TerminalView::new_with_theme(terminal, terminal_theme, window, cx));
        self.configure_terminal_view_silent_mode(tab_id, &view, cx);
        let input_policy = request.input;
        let emit_input_events = match input_policy {
            ByteStreamInputPolicy::CloseOnInterrupt => true,
            ByteStreamInputPolicy::Broadcast => self.tabs[self.active_tab].broadcast_input,
        };
        let input_enabled = self.terminal_input_enabled();
        view.update(cx, |view, cx| {
            view.set_emit_input_events(emit_input_events);
            view.set_input_enabled(input_enabled, cx);
        });
        cx.subscribe_in(
            &view,
            window,
            move |this, _, event, window, cx| match event {
                TerminalViewEvent::Close => this.close_pane(tab_id, pane_id, window, cx),
                TerminalViewEvent::TitleChanged => cx.notify(),
                TerminalViewEvent::Input(input) => match input_policy {
                    ByteStreamInputPolicy::CloseOnInterrupt
                        if ctrl_c_interrupts_byte_stream(input) =>
                    {
                        this.close_pane(tab_id, pane_id, window, cx);
                    }
                    ByteStreamInputPolicy::CloseOnInterrupt => {}
                    ByteStreamInputPolicy::Broadcast => {
                        this.broadcast_input(tab_id, pane_id, input, cx);
                    }
                },
                TerminalViewEvent::OpenEditor(request) => {
                    this.open_editor_in_new_pane(tab_id, pane_id, request.clone(), window, cx);
                }
            },
        )
        .detach();
        let focus_handle = view.focus_handle(cx);
        cx.on_focus(&focus_handle, window, move |this, window, cx| {
            if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.activate_pane(pane_id);
                cx.notify();
            }
            this.clear_active_tab_attention_if_focused(window, cx);
        })
        .detach();
        if let Some(pane) = self.tabs[self.active_tab].pane_mut(pane_id) {
            pane.terminal = Some(view.read(cx).terminal().clone());
            pane.view = Some(view.clone());
        }
        view.focus_handle(cx).focus(window, cx);
        cx.notify();
        Some(pane_id)
    }
}

#[cfg(test)]
#[path = "tests/byte_stream_pane.rs"]
mod tests;
