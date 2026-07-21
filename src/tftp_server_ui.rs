use super::*;

pub(crate) fn tftp_input_stops_server(input: &TerminalInput) -> bool {
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

impl Zetta {
    pub(crate) fn start_tftp_server(
        &mut self,
        _: &StartTftpServer,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .tabs
            .get(self.active_tab)
            .is_some_and(|tab| !can_add_panes(tab.panes.len(), 1))
        {
            self.configuration_error = Some(format!(
                "Could not start TFTP server: this tab has reached the {MAX_PANES_PER_TAB}-pane limit"
            ));
            cx.notify();
            return;
        }
        let root = match self.active_server_root(cx) {
            Ok(root) => root,
            Err(error) => {
                self.configuration_error = Some(format!("Could not start TFTP server: {error:#}"));
                cx.notify();
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let root = root.resolve()?;
                    start_server(&root, DEFAULT_TFTP_PORT)
                })
                .await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(server) => this.open_tftp_server_pane(server, window, cx),
                Err(error) => {
                    this.configuration_error =
                        Some(format!("Could not start TFTP server: {error:#}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn open_tftp_server_pane(
        &mut self,
        server: OpenTftpServer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        if !can_add_panes(tab.panes.len(), 1) {
            return;
        }
        let tab_id = tab.id;
        let active_pane_id = tab.active_pane;
        let Some(profile) = tab.active_profile().cloned() else {
            return;
        };
        let terminal_theme = match resolve_profile_theme(&profile, cx) {
            Ok(theme) => theme,
            Err(error) => {
                self.configuration_error =
                    Some(format!("Could not apply profile theme: {error:#}"));
                cx.notify();
                return;
            }
        };
        let pane_id = self.next_pane_id;
        self.next_pane_id += 1;
        let label = format!("TFTP: {}", server.address);
        let title = format!("TFTP server {} — {}", server.address, server.root.display());

        let tab = &mut self.tabs[self.active_tab];
        tab.maximized_pane = None;
        if !tab
            .layout
            .split(active_pane_id, SplitAxis::Vertical, pane_id)
        {
            return;
        }
        tab.push_pane(TerminalPane {
            id: pane_id,
            label_number: 0,
            generated_label: Some(label),
            custom_label: None,
            profile,
            terminal: None,
            view: None,
            error: None,
            wsl_cwd_file: None,
            pending_command: None,
        });
        tab.activate_pane(pane_id);

        let settings = TerminalSpawnSettings::current(cx);
        let builder = TerminalBuilder::new_byte_stream(
            server.reader,
            server.writer,
            title,
            settings.cursor_shape,
            settings.alternate_scroll,
            settings.max_scroll_history_lines,
            cx.entity_id().as_u64(),
            cx.background_executor(),
            PathStyle::local(),
        );
        let terminal = cx.new(|cx| builder.subscribe(cx));
        let view = cx.new(|cx| TerminalView::new_with_theme(terminal, terminal_theme, window, cx));
        view.update(cx, |view, _| view.set_emit_input_events(true));
        cx.subscribe_in(
            &view,
            window,
            move |this, _, event, window, cx| match event {
                TerminalViewEvent::Close => this.close_pane(tab_id, pane_id, window, cx),
                TerminalViewEvent::TitleChanged => cx.notify(),
                TerminalViewEvent::Input(input) if tftp_input_stops_server(input) => {
                    this.close_pane(tab_id, pane_id, window, cx)
                }
                TerminalViewEvent::Input(_) => {}
            },
        )
        .detach();
        let focus_handle = view.focus_handle(cx);
        cx.on_focus(&focus_handle, window, move |this, _, cx| {
            if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.activate_pane(pane_id);
                cx.notify();
            }
        })
        .detach();
        if let Some(pane) = self.tabs[self.active_tab].pane_mut(pane_id) {
            pane.terminal = Some(view.read(cx).terminal().clone());
            pane.view = Some(view.clone());
        }
        view.focus_handle(cx).focus(window, cx);
        cx.notify();
    }
}

#[cfg(test)]
#[path = "tests/tftp_server_ui.rs"]
mod tests;
