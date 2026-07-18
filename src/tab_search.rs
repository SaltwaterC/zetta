use super::*;

#[derive(Clone, Copy)]
pub(crate) struct TabSearchMatch {
    pub(crate) pane_id: u64,
    pub(crate) match_index: usize,
}

pub(crate) struct TabSearch {
    pub(crate) tab_id: u64,
    pub(crate) query: String,
    pub(crate) cursor: usize,
    pub(crate) select_all: bool,
    pub(crate) generation: u64,
    pub(crate) matches: Vec<TabSearchMatch>,
    pub(crate) active_match: Option<usize>,
    pub(crate) task: Option<Task<()>>,
}

pub(crate) fn tab_search_request_is_current(
    search: Option<&TabSearch>,
    tab_id: u64,
    generation: u64,
    query: &str,
) -> bool {
    search.is_some_and(|search| {
        search.tab_id == tab_id && search.generation == generation && search.query == query
    })
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
                .filter_map(|pane| pane.view.clone())
                .collect::<Vec<_>>();
            for view in views {
                view.update(cx, TerminalView::clear_search);
            }
            self.command_palette = None;
            self.tab_search = Some(TabSearch {
                tab_id,
                query: String::new(),
                cursor: 0,
                select_all: false,
                generation: 0,
                matches: Vec::new(),
                active_match: None,
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
            .filter_map(|pane| pane.view.as_ref())
            .map(|view| view.read(cx).terminal().clone())
            .collect::<Vec<_>>();
        for terminal in terminals {
            terminal.update(cx, |terminal, _| {
                Arc::make_mut(&mut terminal.matches).clear()
            });
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

    pub(crate) fn refresh_tab_search(&mut self, cx: &mut Context<Self>) {
        let Some(search_state) = self.tab_search.as_mut() else {
            return;
        };
        search_state.task.take();
        search_state.generation = search_state.generation.wrapping_add(1);
        search_state.matches.clear();
        search_state.active_match = None;
        let tab_id = search_state.tab_id;
        let query = search_state.query.clone();
        let generation = search_state.generation;

        let terminals = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .into_iter()
            .flat_map(|tab| tab.panes.iter())
            .filter_map(|pane| {
                pane.view
                    .as_ref()
                    .map(|view| (pane.id, view.read(cx).terminal().clone()))
            })
            .collect::<Vec<_>>();
        for (_, terminal) in &terminals {
            terminal.update(cx, |terminal, _| {
                Arc::make_mut(&mut terminal.matches).clear()
            });
        }
        if query.is_empty() {
            cx.notify();
            return;
        }
        let Some(pattern) = Search::new(&regex::escape(&query)) else {
            return;
        };
        let executor = cx.background_executor().clone();
        let task = cx.spawn(async move |this, cx| {
            executor.timer(Duration::from_millis(75)).await;
            let Some(tasks) = this
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
                            .map(|(pane_id, terminal)| {
                                let task = terminal.update(cx, |terminal, cx| {
                                    terminal.find_matches(pattern.clone(), cx)
                                });
                                (pane_id, terminal, task)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .ok()
                .flatten()
            else {
                return;
            };
            let mut results = Vec::with_capacity(tasks.len());
            for (pane_id, terminal, task) in tasks {
                let matches: Vec<Range> = task.await;
                results.push((pane_id, terminal, matches));
            }
            this.update(cx, |this, cx| {
                let valid = tab_search_request_is_current(
                    this.tab_search.as_ref(),
                    tab_id,
                    generation,
                    &query,
                );
                if !valid {
                    return;
                }

                let mut aggregated = Vec::new();
                for (pane_id, terminal, matches) in results {
                    let match_count = matches.len();
                    terminal.update(cx, |terminal, _| terminal.matches = Arc::new(matches));
                    aggregated.extend((0..match_count).map(|match_index| TabSearchMatch {
                        pane_id,
                        match_index,
                    }));
                }
                let active_match = aggregated.len().checked_sub(1);
                if let Some(search) = this.tab_search.as_mut() {
                    search.matches = aggregated;
                    search.active_match = active_match;
                }
                if let Some(index) = active_match {
                    this.activate_tab_search_match(index, cx);
                }
                cx.notify();
            })
            .ok();
        });
        if let Some(search) = self.tab_search.as_mut() {
            search.task = Some(task);
        }
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
        tab.activate_pane(search_match.pane_id);
        let terminal = tab
            .pane(search_match.pane_id)
            .and_then(|pane| pane.view.as_ref())
            .map(|view| view.read(cx).terminal().clone());
        if let Some(terminal) = terminal {
            terminal.update(cx, |terminal, _| {
                terminal.activate_match(search_match.match_index)
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

    pub(crate) fn insert_tab_search_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(search) = self.tab_search.as_mut() else {
            return;
        };
        if search.select_all {
            search.query.clear();
            search.cursor = 0;
        }
        search.query.insert_str(search.cursor, text);
        search.cursor += text.len();
        search.select_all = false;
        self.refresh_tab_search(cx);
    }

    pub(crate) fn tab_search_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_tab_search(window, cx),
            "enter" | "f3" if event.keystroke.modifiers.shift => self.navigate_tab_search(true, cx),
            "enter" | "f3" => self.navigate_tab_search(false, cx),
            "backspace" => {
                if let Some(search) = self.tab_search.as_mut() {
                    if search.select_all {
                        search.query.clear();
                        search.cursor = 0;
                    } else if search.cursor > 0 {
                        let previous = previous_char_boundary(&search.query, search.cursor);
                        search.query.replace_range(previous..search.cursor, "");
                        search.cursor = previous;
                    }
                    search.select_all = false;
                }
                self.refresh_tab_search(cx);
            }
            "delete" => {
                if let Some(search) = self.tab_search.as_mut() {
                    if search.select_all {
                        search.query.clear();
                        search.cursor = 0;
                    } else if search.cursor < search.query.len() {
                        let next = next_char_boundary(&search.query, search.cursor);
                        search.query.replace_range(search.cursor..next, "");
                    }
                    search.select_all = false;
                }
                self.refresh_tab_search(cx);
            }
            "left" => {
                if let Some(search) = self.tab_search.as_mut() {
                    search.cursor = if search.select_all {
                        0
                    } else {
                        previous_char_boundary(&search.query, search.cursor)
                    };
                    search.select_all = false;
                }
                cx.notify();
            }
            "right" => {
                if let Some(search) = self.tab_search.as_mut() {
                    search.cursor = if search.select_all {
                        search.query.len()
                    } else {
                        next_char_boundary(&search.query, search.cursor)
                    };
                    search.select_all = false;
                }
                cx.notify();
            }
            "home" => {
                if let Some(search) = self.tab_search.as_mut() {
                    search.cursor = 0;
                    search.select_all = false;
                }
                cx.notify();
            }
            "end" => {
                if let Some(search) = self.tab_search.as_mut() {
                    search.cursor = search.query.len();
                    search.select_all = false;
                }
                cx.notify();
            }
            "a" if event.keystroke.modifiers.control || event.keystroke.modifiers.platform => {
                if let Some(search) = self.tab_search.as_mut() {
                    search.select_all = !search.query.is_empty();
                }
                cx.notify();
            }
            "v" if event.keystroke.modifiers.control || event.keystroke.modifiers.platform => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.insert_tab_search_text(&text, cx);
                }
            }
            _ if !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.alt =>
            {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    self.insert_tab_search_text(text, cx);
                }
            }
            _ => {}
        }
        cx.stop_propagation();
    }
}

#[cfg(test)]
#[path = "tests/tab_search.rs"]
mod tests;
