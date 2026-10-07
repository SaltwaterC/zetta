use super::*;

#[derive(Clone, Copy)]
pub(crate) struct TabSearchMatch {
    pub(crate) pane_id: u64,
    pub(crate) stack_id: Option<u64>,
    pub(crate) match_index: usize,
}

pub(crate) struct TabSearch {
    pub(crate) tab_id: u64,
    pub(crate) query: TextField,
    pub(crate) generation: u64,
    pub(crate) matches: Vec<TabSearchMatch>,
    pub(crate) active_match: Option<usize>,
    pub(crate) limit_reached: bool,
    pub(crate) total_count: usize,
    /// Every pane's search has read all of its scrollback, so the count is exact.
    pub(crate) complete: bool,
    pub(crate) task: Option<Task<()>>,
}

/// One pane's part of a running tab search: what its search last reported.
struct TabSearchPane {
    pane_id: u64,
    stack_id: Option<u64>,
    terminal: Entity<Terminal>,
    shown: usize,
    total_count: usize,
    limit_reached: bool,
    complete: bool,
}

impl TabSearchPane {
    fn is(&self, search_match: TabSearchMatch) -> bool {
        self.pane_id == search_match.pane_id && self.stack_id == search_match.stack_id
    }
}

pub(crate) fn tab_search_request_is_current(
    search: Option<&TabSearch>,
    tab_id: u64,
    generation: u64,
    query: &str,
) -> bool {
    search.is_some_and(|search| {
        search.tab_id == tab_id && search.generation == generation && search.query.text == query
    })
}

pub(crate) fn tab_search_targets_tab(search: Option<&TabSearch>, tab_id: u64) -> bool {
    search.is_some_and(|search| search.tab_id == tab_id)
}

/// Drops a terminal's highlighted matches. Notifying the terminal is what
/// repaints the panes: the highlights come from terminal state, so the search
/// overlay's own `cx.notify()` doesn't cover them.
fn clear_terminal_matches(terminal: &Entity<Terminal>, cx: &mut Context<Zetta>) {
    terminal.update(cx, |terminal, cx| {
        if terminal.matches.is_empty() {
            return;
        }
        Arc::make_mut(&mut terminal.matches).clear();
        cx.notify();
    });
}

impl Zetta {
    pub(crate) fn search_tab_scrollback(
        &mut self,
        _: &SearchTabScrollback,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tab_search.is_none() {
            let Some(tab) = self.tabs.get(self.active_tab) else {
                return;
            };
            let tab_id = tab.id;
            let views = tab
                .panes
                .iter()
                .flat_map(TerminalPane::all_views)
                .cloned()
                .collect::<Vec<_>>();
            for view in views {
                view.update(cx, TerminalView::clear_search);
            }
            self.command_palette = None;
            self.tab_search = Some(TabSearch {
                tab_id,
                query: TextField::default(),
                generation: 0,
                matches: Vec::new(),
                active_match: None,
                limit_reached: false,
                total_count: 0,
                complete: true,
                task: None,
            });
            self.refresh_tab_search(cx);
        }
        self.tab_search_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn clear_tab_search_matches(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let terminals = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .into_iter()
            .flat_map(|tab| tab.panes.iter())
            .flat_map(TerminalPane::all_terminals)
            .cloned()
            .collect::<Vec<_>>();
        for terminal in terminals {
            clear_terminal_matches(&terminal, cx);
        }
    }

    pub(crate) fn dismiss_tab_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(search) = self.tab_search.take() else {
            return;
        };
        self.clear_tab_search_matches(search.tab_id, cx);
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn cancel_tab_search_for_tab(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        if !tab_search_targets_tab(self.tab_search.as_ref(), tab_id) {
            return;
        }
        let search = self.tab_search.take().expect("targeted search must exist");
        self.clear_tab_search_matches(search.tab_id, cx);
        cx.notify();
    }

    pub(crate) fn refresh_tab_search(&mut self, cx: &mut Context<Self>) {
        let Some(search_state) = self.tab_search.as_mut() else {
            return;
        };
        search_state.task.take();
        search_state.generation = search_state.generation.wrapping_add(1);
        search_state.matches.clear();
        search_state.active_match = None;
        search_state.limit_reached = false;
        search_state.total_count = 0;
        search_state.complete = search_state.query.text.is_empty();
        let tab_id = search_state.tab_id;
        let query = search_state.query.text.clone();
        let generation = search_state.generation;

        let terminals = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .into_iter()
            .flat_map(|tab| tab.panes.iter())
            .flat_map(|pane| {
                pane.terminal
                    .iter()
                    .map(move |terminal| (pane.id, None, terminal.clone()))
                    .chain(pane.stack.entries.iter().filter_map(move |entry| {
                        Some((pane.id, Some(entry.id), entry.terminal.clone()?))
                    }))
            })
            .collect::<Vec<_>>();
        for (_, _, terminal) in &terminals {
            clear_terminal_matches(terminal, cx);
        }
        if query.is_empty() {
            cx.notify();
            return;
        }
        let Some(pattern) = Search::new_literal(&query) else {
            return;
        };
        let executor = cx.background_executor().clone();
        let task = cx.spawn(async move |this, cx| {
            executor.timer(Duration::from_millis(75)).await;
            let Some(jobs) = this
                .update(cx, |this, cx| {
                    let valid = tab_search_request_is_current(
                        this.tab_search.as_ref(),
                        tab_id,
                        generation,
                        &query,
                    );
                    valid.then(|| {
                        terminals
                            .into_iter()
                            .map(|(pane_id, stack_id, terminal)| {
                                let job = terminal.update(cx, |terminal, cx| {
                                    terminal.find_matches(pattern.clone(), cx)
                                });
                                let pane = TabSearchPane {
                                    pane_id,
                                    stack_id,
                                    terminal,
                                    shown: 0,
                                    total_count: 0,
                                    limit_reached: false,
                                    complete: false,
                                };
                                (pane, job)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .ok()
                .flatten()
            else {
                return;
            };
            let (mut panes, jobs): (Vec<_>, Vec<_>) = jobs.into_iter().unzip();
            // Every pane's results as they arrive, tagged with the pane they belong to.
            let mut updates =
                futures::stream::select_all(jobs.into_iter().enumerate().map(|(index, job)| {
                    Box::pin(futures::stream::unfold(job, move |mut job| async move {
                        let result = job.next().await?;
                        Some(((index, result), job))
                    }))
                }));
            while let Some((index, result)) = futures::StreamExt::next(&mut updates).await {
                let current = this
                    .update(cx, |this, cx| {
                        let current = tab_search_request_is_current(
                            this.tab_search.as_ref(),
                            tab_id,
                            generation,
                            &query,
                        );
                        if current {
                            this.apply_tab_search_results(&mut panes, index, result, cx);
                        }
                        current
                    })
                    .unwrap_or(false);
                if !current {
                    return;
                }
            }
        });
        if let Some(search) = self.tab_search.as_mut() {
            search.task = Some(task);
        }
    }

    /// Fold pane `index`'s latest results into the tab search. The active match stays where it
    /// is; the first match found becomes active when there is none.
    fn apply_tab_search_results(
        &mut self,
        panes: &mut [TabSearchPane],
        index: usize,
        result: SearchMatches,
        cx: &mut Context<Self>,
    ) {
        let Some(search) = self.tab_search.as_mut() else {
            return;
        };
        let active = search
            .active_match
            .and_then(|active| search.matches.get(active).copied());
        let pane = &mut panes[index];
        let previous = pane.shown;
        pane.shown = result.ranges.len();
        pane.total_count = result.total_count;
        pane.limit_reached = result.limit_reached;
        pane.complete = result.complete;
        let carried = active
            .filter(|&active| pane.is(active))
            .map(|active| TabSearchMatch {
                match_index: result.carried_index(active.match_index, previous),
                ..active
            });
        pane.terminal.update(cx, |terminal, cx| {
            terminal.matches = Arc::new(result.ranges);
            // The match highlights are painted from the terminal's own state, so the terminal
            // has to repaint even though this update was driven by the search overlay.
            cx.notify();
        });

        search.matches = panes
            .iter()
            .flat_map(|pane| {
                (0..pane.shown).map(|match_index| TabSearchMatch {
                    pane_id: pane.pane_id,
                    stack_id: pane.stack_id,
                    match_index,
                })
            })
            .collect();
        let activate = active.is_none() && !search.matches.is_empty();
        search.active_match = match carried.or(active) {
            Some(active) => search.matches.iter().position(|search_match| {
                search_match.pane_id == active.pane_id
                    && search_match.stack_id == active.stack_id
                    && search_match.match_index == active.match_index
            }),
            None => search.matches.len().checked_sub(1),
        };
        search.limit_reached = panes.iter().any(|pane| pane.limit_reached);
        search.total_count = panes
            .iter()
            .fold(0, |total, pane| total.saturating_add(pane.total_count));
        search.complete = panes.iter().all(|pane| pane.complete);
        if activate && let Some(index) = search.active_match {
            self.activate_tab_search_match(index, cx);
        }
        cx.notify();
    }

    pub(crate) fn activate_tab_search_match(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((tab_id, search_match)) = self.tab_search.as_ref().and_then(|search| {
            search
                .matches
                .get(index)
                .copied()
                .map(|search_match| (search.tab_id, search_match))
        }) else {
            return;
        };
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        tab.restore_minimized(search_match.pane_id);
        tab.maximized_pane = None;
        tab.activate_stack_entry(
            search_match.pane_id,
            search_match
                .stack_id
                .map_or(PaneStackSelection::Base, PaneStackSelection::Stacked),
        );
        let terminal = tab.active_terminal();
        if let Some(terminal) = terminal {
            terminal.update(cx, |terminal, cx| {
                terminal.activate_match(search_match.match_index);
                cx.notify();
            });
        }
        cx.notify();
    }

    pub(crate) fn navigate_tab_search(&mut self, previous: bool, cx: &mut Context<Self>) {
        let Some(search) = self.tab_search.as_mut() else {
            return;
        };
        let match_count = search.matches.len();
        if match_count == 0 {
            return;
        }
        let current = search
            .active_match
            .unwrap_or(if previous { 0 } else { match_count - 1 });
        let index = if previous {
            current.checked_sub(1).unwrap_or(match_count - 1)
        } else {
            (current + 1) % match_count
        };
        search.active_match = Some(index);
        self.activate_tab_search_match(index, cx);
    }

    pub(crate) fn tab_search_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Settled before this overlay's own keys, so `Ctrl-X` cuts rather than
        // typing an `x` and `Shift-Delete` cuts rather than forward-deleting.
        if let Some(search) = self.tab_search.as_mut() {
            match apply_clipboard_shortcut(&mut search.query, &event.keystroke, cx) {
                ClipboardOutcome::Ignored => {}
                ClipboardOutcome::Unchanged => {
                    cx.notify();
                    return;
                }
                ClipboardOutcome::Edited => {
                    // The query drives the match list, so a cut or a paste has to
                    // run the search again rather than only redraw.
                    self.refresh_tab_search(cx);
                    return;
                }
            }
        }
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_tab_search(window, cx),
            "enter" | "f3" if event.keystroke.modifiers.shift => self.navigate_tab_search(true, cx),
            "enter" | "f3" => self.navigate_tab_search(false, cx),
            _ => {
                let edited = self
                    .tab_search
                    .as_mut()
                    .map(|search| apply_text_field_key(&mut search.query, &event.keystroke));
                match edited {
                    Some(TextFieldEdit::CursorMoved) => cx.notify(),
                    // The query drives the match list, so an edit has to run the
                    // search again rather than only redraw.
                    Some(TextFieldEdit::Edited) => self.refresh_tab_search(cx),
                    Some(TextFieldEdit::Ignored) | None => {}
                }
            }
        }
        cx.stop_propagation();
    }

    /// The scrollback search field anchored below the title bar.
    pub(crate) fn render_tab_search_overlay(
        &self,
        colors: &ThemeColors,
        top_inset: Pixels,
    ) -> Option<AnyElement> {
        let search = self.tab_search.as_ref()?;
        let query = field_query_run(&search.query, Some("Search all panes…"), colors);
        let status = SearchStatus {
            active_match: search.active_match,
            shown: search.matches.len(),
            total_count: search.total_count,
            limit_reached: search.limit_reached,
            complete: search.complete,
        }
        .label();

        Some(
            div()
                .absolute()
                .top(top_inset)
                .left_2()
                .right_2()
                .flex()
                .justify_end()
                .child(
                    div()
                        .id("tab-scrollback-search")
                        .track_focus(&self.tab_search_focus)
                        .w_full()
                        .max_w(px(460.0))
                        .px_3()
                        .py_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .rounded(crate::ui_tokens::RADIUS_SURFACE)
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background.alpha(1.0))
                        .shadow_sm()
                        .text_sm()
                        .text_color(colors.text)
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .gap_3()
                                .child(query)
                                .child(
                                    div()
                                        .flex_none()
                                        .text_color(colors.text_muted)
                                        .child(status),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child(key_hints(&[
                                    ("Enter", "next"),
                                    ("Shift+Enter", "previous"),
                                    ("Esc", "close"),
                                ])),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
#[path = "tests/tab_search.rs"]
mod tests;
