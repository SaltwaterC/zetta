//! Adopting a console Windows handed to Zetta as the default terminal.

use super::*;

impl Zetta {
    pub(crate) fn spawn_windows_handoff_terminal(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        profile: Profile,
        request: crate::windows_integration::WindowsHandoffRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal_theme) = self.resolve_pane_spawn_theme(tab_id, pane_id, &profile, cx)
        else {
            return;
        };
        let mut settings = TerminalSpawnSettings::current(cx);
        let path_hyperlink_regexes = settings.path_hyperlink_regexes(true);
        let child_handle = match request.duplicate_child_handle() {
            Ok(handle) => handle,
            Err(error) => {
                self.report_pane_spawn_error(
                    tab_id,
                    pane_id,
                    format!("Could not monitor the handed-over process: {error}"),
                    cx,
                );
                return;
            }
        };
        let options = terminal::AttachedOptions {
            shell: profile.command.clone(),
            env: native_terminal_environment().into_iter().collect(),
            cursor_shape: settings.cursor_shape,
            alternate_scroll: settings.alternate_scroll,
            max_scroll_history_lines: settings.max_scroll_history_lines,
            path_hyperlink_regexes,
            path_hyperlink_timeout_ms: settings.path_hyperlink_timeout_ms,
            window_id: cx.entity_id().as_u64(),
        };
        let image_paste_handler = Arc::new(crate::ssh_image_paste::SshImagePasteHandler::new(
            options.shell.clone(),
            options.env.clone(),
            None,
        ));
        let handover = request.into_handover();
        let run_identity = self.run_pane_identity(tab_id, pane_id);
        let build_executor = cx.background_executor().clone();
        let terminal_executor = build_executor.clone();
        let build = build_executor.spawn(async move {
            TerminalBuilder::new_attached(handover, options, &terminal_executor, PathStyle::local())
        });
        let this = cx.entity().downgrade();
        let terminal_theme_for_task = terminal_theme.clone();
        window
            .spawn(cx, async move |cx| match build.await {
                Ok(attached) => {
                    let terminal::AttachedTerminal {
                        builder,
                        child_events,
                    } = attached;
                    let builder = builder.with_image_paste_handler(image_paste_handler);
                    crate::windows_integration::monitor_handoff_child(child_handle, child_events);
                    this.update_in(cx, |this, window, cx| {
                        let terminal = cx.new(|cx| builder.subscribe(cx));
                        let view = cx.new(|cx| {
                            TerminalView::new_with_theme(
                                terminal.clone(),
                                terminal_theme_for_task,
                                window,
                                cx,
                            )
                        });
                        this.configure_terminal_view_silent_mode(tab_id, &view, cx);
                        this.subscribe_spawned_terminal(
                            tab_id,
                            pane_id,
                            run_identity,
                            &terminal,
                            window,
                            cx,
                        );
                        this.subscribe_spawned_terminal_view(tab_id, pane_id, &view, window, cx);
                        let should_focus = this
                            .configure_spawned_terminal_focus(tab_id, pane_id, &view, window, cx);
                        let tab_index = this.tab_index(tab_id);
                        if let Some(pane) = tab_index
                            .and_then(|index| this.tabs.get_mut(index))
                            .and_then(|tab| tab.pane_mut(pane_id))
                        {
                            pane.terminal = Some(terminal.clone());
                            pane.view = Some(view.clone());
                            pane.base_exited = false;
                            pane.error = None;
                            pane.exit = None;
                        }
                        this.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
                        this.schedule_project_detection_for_pane(tab_id, pane_id, window, cx);
                        if should_focus {
                            let focus_handle = view.focus_handle(cx);
                            this.focus_terminal_if_allowed(&focus_handle, window, cx);
                        }
                        this.sync_visible_terminals(cx);
                        this.schedule_terminal_spawn_notify(cx);
                    })
                    .ok();
                }
                Err(error) => {
                    this.update_in(cx, |this, _, cx| {
                        if let Some(pane) = this.tabs.pane_in_mut(tab_id, pane_id) {
                            pane.error = Some(format!("{error:#}"));
                        }
                        this.schedule_terminal_spawn_notify(cx);
                    })
                    .ok();
                }
            })
            .detach();
    }
}
