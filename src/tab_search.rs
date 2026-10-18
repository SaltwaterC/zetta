//! Tab search presentation and pane/stack revelation. Search lifecycle and
//! navigation belong to `terminal::search_session`, shared with pane search.

use super::*;
use terminal::search_session::{SearchActivated, SearchMatch, SearchSession, SearchSessionStatus};

pub(crate) struct TabSearch {
    pub(crate) tab_id: u64,
    pub(crate) query: TextField,
    session: Entity<SearchSession>,
    targets: Vec<(u64, Option<u64>)>,
    status: SearchStatus,
    _observer: Subscription,
    _activation: Subscription,
}

impl TabSearch {
    fn new(tab_id: u64, cx: &mut Context<Zetta>) -> Self {
        let session = cx.new(|cx| SearchSession::new(Vec::new(), cx));
        let observer = cx.observe(&session, |this, session, cx| {
            if let Some(search) = this
                .tab_search
                .as_mut()
                .filter(|search| search.session == session)
            {
                search.status = session.read(cx).status().into();
                cx.notify();
            }
        });
        let activation = cx.subscribe(&session, |this, session, event: &SearchActivated, cx| {
            if this
                .tab_search
                .as_ref()
                .is_some_and(|search| search.session == session)
            {
                this.reveal_tab_search_match(event.0, cx);
            }
        });
        Self {
            tab_id,
            query: TextField::default(),
            session,
            targets: Vec::new(),
            status: SearchSessionStatus::default().into(),
            _observer: observer,
            _activation: activation,
        }
    }
}

pub(crate) fn tab_search_targets_tab(search: Option<&TabSearch>, tab_id: u64) -> bool {
    search.is_some_and(|search| search.tab_id == tab_id)
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
            self.tab_search = Some(TabSearch::new(tab_id, cx));
            self.refresh_tab_search(cx);
        }
        self.tab_search_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn dismiss_tab_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(search) = self.tab_search.take() else {
            return;
        };
        search.session.update(cx, SearchSession::clear);
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn cancel_tab_search_for_tab(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        if !tab_search_targets_tab(self.tab_search.as_ref(), tab_id) {
            return;
        }
        let search = self.tab_search.take().expect("targeted search must exist");
        search.session.update(cx, SearchSession::clear);
        cx.notify();
    }

    pub(crate) fn refresh_tab_search(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.tab_search.as_ref() else {
            return;
        };
        let tab_id = search.tab_id;
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
        let Some(search) = self.tab_search.as_mut() else {
            return;
        };
        search.targets = terminals
            .iter()
            .map(|(pane, stack, _)| (*pane, *stack))
            .collect();
        search.session.update(cx, |session, cx| {
            session.set_targets(
                terminals
                    .into_iter()
                    .map(|(_, _, terminal)| terminal)
                    .collect(),
                cx,
            );
            session.set_query(&search.query.text, cx);
        });
        search.status = search.session.read(cx).status().into();
        cx.notify();
    }

    fn reveal_tab_search_match(&mut self, found: SearchMatch, cx: &mut Context<Self>) {
        let Some((tab_id, pane_id, stack_id)) = self.tab_search.as_ref().and_then(|search| {
            search
                .targets
                .get(found.target)
                .map(|&(pane, stack)| (search.tab_id, pane, stack))
        }) else {
            return;
        };
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        tab.restore_minimized(pane_id);
        tab.maximized_pane = None;
        tab.activate_stack_entry(
            pane_id,
            stack_id.map_or(PaneStackSelection::Base, PaneStackSelection::Stacked),
        );
        cx.notify();
    }

    pub(crate) fn navigate_tab_search(&mut self, previous: bool, cx: &mut Context<Self>) {
        if let Some(search) = &self.tab_search {
            search
                .session
                .update(cx, |session, cx| session.navigate(previous, cx));
        }
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
        let query = field_query_run(&search.query, Some("Search all panes…"), colors)
            .flex_1()
            .flex_basis(px(120.))
            .min_w_0()
            .overflow_hidden()
            .debug_selector(|| format!("tab-search-query:{}", search.query.text));
        let status = search.status.label();

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
                                .flex_wrap()
                                .justify_between()
                                .gap_3()
                                .child(query)
                                .child(
                                    div()
                                        .flex_none()
                                        .max_w_full()
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
