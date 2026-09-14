//! Shared state, keyboard handling, and rendering for searchable dropdowns.
//!
//! The settings editor and the remote-session picker own different option
//! lists and commit different values, but their dropdown interaction is the
//! same. Keeping that interaction here prevents the two surfaces from
//! drifting apart.

use super::*;

/// Seven dropdown rows plus the list's two 4px padding edges fit exactly in
/// the 260px viewport, so the virtualized list never paints a partial row.
pub(crate) const DROPDOWN_OPTION_ROW_HEIGHT: Pixels = px(36.);
pub(crate) const DROPDOWN_OPTIONS_MAX_HEIGHT: Pixels = px(260.);
pub(crate) const DROPDOWN_LIST_VIEWPORT_HEIGHT: Pixels = px(252.);

/// The result of a key pressed while a searchable dropdown is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SearchableDropdownAction {
    /// The key was consumed without changing the open/closed state. This
    /// includes arrows and query edits.
    Handled,
    /// The popup should close without changing the value behind it.
    Close,
    /// The highlighted value should be committed. `None` means that the query
    /// has no matches and the popup must stay open.
    Commit(Option<String>),
    /// The popup should close and the owning surface should advance focus.
    Tab { reverse: bool },
}

/// Search/filter/navigation state for one open searchable dropdown.
#[derive(Clone)]
pub(crate) struct SearchableDropdown {
    pub(crate) selected_index: usize,
    pub(crate) query: String,
    pub(crate) options: Arc<[String]>,
    /// Option indices in display order after applying `query`.
    pub(crate) rows: Arc<[usize]>,
    /// Row index (not option index) of the widest visible option.
    pub(crate) widest_row: Option<usize>,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) anchor: Point<Pixels>,
}

impl Default for SearchableDropdown {
    fn default() -> Self {
        Self {
            selected_index: 0,
            query: String::new(),
            options: Arc::from([]),
            rows: Arc::from([]),
            widest_row: None,
            scroll: UniformListScrollHandle::new(),
            anchor: Point::default(),
        }
    }
}

impl SearchableDropdown {
    /// Opens the dropdown on `selected_index` without changing that selection.
    pub(crate) fn open(
        &mut self,
        options: Arc<[String]>,
        selected_index: usize,
        anchor: Point<Pixels>,
    ) -> bool {
        if options.is_empty() {
            return false;
        }
        self.options = options;
        self.selected_index = selected_index.min(self.options.len() - 1);
        self.query.clear();
        self.anchor = anchor;
        self.refresh_snapshot();
        self.scroll_to_selection();
        true
    }

    /// Replaces the option snapshot while preserving the query and as much of
    /// the current selection as the new list can hold.
    pub(crate) fn set_options(&mut self, options: Arc<[String]>) {
        self.options = options;
        self.selected_index = self
            .selected_index
            .min(self.options.len().saturating_sub(1));
        self.refresh_snapshot();
        self.scroll_to_selection();
    }

    pub(crate) fn close(&mut self) {
        self.query.clear();
        self.rows = Arc::from([]);
        self.widest_row = None;
    }

    pub(crate) fn refresh_snapshot(&mut self) {
        let (rows, widest_row) = dropdown_snapshot_rows(&self.options, &self.query);
        self.rows = rows;
        self.widest_row = widest_row;
    }

    pub(crate) fn scroll_to_selection(&mut self) {
        let Some(row) = self
            .rows
            .iter()
            .position(|index| *index == self.selected_index)
        else {
            return;
        };
        self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
    }

    pub(crate) fn move_selection(&mut self, direction: i32) -> bool {
        if self.rows.is_empty() {
            return false;
        }
        let current = self
            .rows
            .iter()
            .position(|index| *index == self.selected_index)
            .unwrap_or(0);
        let next = if direction < 0 {
            current.checked_sub(1).unwrap_or(self.rows.len() - 1)
        } else {
            (current + 1) % self.rows.len()
        };
        self.selected_index = self.rows[next];
        self.scroll_to_selection();
        true
    }

    #[cfg(test)]
    pub(crate) fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
        if let Some(index) = fuzzy_match_index(&self.options, &self.query) {
            self.selected_index = index;
        }
        self.refresh_snapshot();
        self.scroll_to_selection();
    }

    pub(crate) fn selected_value(&self) -> Option<String> {
        if !self.query.is_empty() && !self.rows.contains(&self.selected_index) {
            return None;
        }
        self.options.get(self.selected_index).cloned()
    }

    /// Applies the shared dropdown key contract. The caller decides what a
    /// committed value means and how focus advances after Tab.
    pub(crate) fn key_down(
        &mut self,
        event: &KeyDownEvent,
        command: bool,
    ) -> SearchableDropdownAction {
        match event.keystroke.key.as_str() {
            "escape" => SearchableDropdownAction::Close,
            "tab" => SearchableDropdownAction::Tab {
                reverse: event.keystroke.modifiers.shift,
            },
            "up" | "left" => {
                self.move_selection(-1);
                SearchableDropdownAction::Handled
            }
            "down" | "right" => {
                self.move_selection(1);
                SearchableDropdownAction::Handled
            }
            "enter" | "space" => SearchableDropdownAction::Commit(self.selected_value()),
            "backspace" => {
                self.query.pop();
                self.after_query_edit();
                SearchableDropdownAction::Handled
            }
            _ if !command
                && !event.keystroke.modifiers.alt
                && event
                    .keystroke
                    .key_char
                    .as_ref()
                    .is_some_and(|text| !text.chars().any(char::is_control)) =>
            {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    self.query.push_str(text);
                    self.after_query_edit();
                }
                SearchableDropdownAction::Handled
            }
            _ => SearchableDropdownAction::Handled,
        }
    }

    fn after_query_edit(&mut self) {
        if let Some(index) = fuzzy_match_index(&self.options, &self.query) {
            self.selected_index = index;
        }
        self.refresh_snapshot();
        self.scroll_to_selection();
    }

    pub(crate) fn render_state(&self) -> SearchableDropdownRenderState {
        SearchableDropdownRenderState {
            selected_index: self.selected_index,
            query: self.query.clone(),
            options: self.options.clone(),
            rows: self.rows.clone(),
            widest_row: self.widest_row,
            scroll: self.scroll.clone(),
            anchor: self.anchor,
        }
    }
}

/// Owned render snapshot for a dropdown popup. Keeping this separate from the
/// mutable state lets a row closure own all of its data for GPUI's virtualized
/// list without borrowing a settings form or picker.
#[derive(Clone)]
pub(crate) struct SearchableDropdownRenderState {
    pub(crate) selected_index: usize,
    pub(crate) query: String,
    pub(crate) options: Arc<[String]>,
    pub(crate) rows: Arc<[usize]>,
    pub(crate) widest_row: Option<usize>,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) anchor: Point<Pixels>,
}

/// The rows visible in an open dropdown, together with the row used to measure
/// its intrinsic width.
pub(crate) fn dropdown_snapshot_rows(
    options: &[String],
    query: &str,
) -> (Arc<[usize]>, Option<usize>) {
    let rows: Arc<[usize]> = if query.is_empty() {
        (0..options.len()).collect::<Vec<_>>().into()
    } else {
        fuzzy_match_indices(options, query).into()
    };
    let widest_row = rows
        .iter()
        .enumerate()
        .max_by_key(|(_, index)| options[**index].chars().count())
        .map(|(row, _)| row);
    (rows, widest_row)
}

/// Renders the anchored searchable popup. Callers provide only the option
/// decoration and the action performed after a row is committed.
pub(crate) fn searchable_dropdown_popup<Leading, Select>(
    id: String,
    colors: ThemeColors,
    state: SearchableDropdownRenderState,
    render_leading: Leading,
    on_select: Select,
) -> AnyElement
where
    Leading: Fn(&str, &ThemeColors) -> Option<AnyElement> + Clone + 'static,
    Select: Fn(String, &mut App) + Clone + 'static,
{
    let options = state.options.clone();
    let active_index = state.selected_index.min(options.len().saturating_sub(1));
    let query = state.query.clone();
    let row_indices = state.rows.clone();
    let widest_row = state.widest_row;
    let no_matches = row_indices.is_empty();
    let list_colors = colors.clone();
    let list_id = id.clone();
    let options_region_selector = format!("{id}-options-region");
    let search_selector = format!("{id}-search-banner");
    let no_matches_selector = format!("{id}-no-matches");
    let popup_selector = format!("{id}-options");
    let option_rows = uniform_list(
        format!("{id}-options-list"),
        row_indices.len(),
        move |range: std::ops::Range<usize>, _, _| {
            range
                .map(|row| {
                    let index = row_indices[row];
                    let value = options[index].clone();
                    let selected = index == active_index;
                    let leading = render_leading.clone();
                    let on_select = on_select.clone();
                    let row_selector = format!("{list_id}-option-{index}");
                    div()
                        .id(format!("{list_id}-option-{index}"))
                        .debug_selector(move || row_selector.clone())
                        .when(index == 0, |row| {
                            row.debug_selector(|| "dropdown-first-option".to_owned())
                        })
                        .when(index == 1, |row| {
                            row.debug_selector(|| "dropdown-second-option".to_owned())
                        })
                        .when(index == 6, |row| {
                            row.debug_selector(|| "dropdown-seventh-option".to_owned())
                        })
                        .when(index == 7, |row| {
                            row.debug_selector(|| "dropdown-eighth-option".to_owned())
                        })
                        .h(DROPDOWN_OPTION_ROW_HEIGHT)
                        .px_2()
                        .py_1()
                        .rounded(px(3.))
                        .cursor_pointer()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .when(selected, |row| row.bg(list_colors.element_selected))
                        .hover(|style| style.bg(list_colors.element_hover))
                        .child(
                            h_flex()
                                .gap_2()
                                .when_some(leading(&value, &list_colors), |row, leading| {
                                    row.child(leading)
                                })
                                .child(value.clone()),
                        )
                        .on_click(move |_, _, cx| on_select(value.clone(), cx))
                })
                .collect::<Vec<_>>()
        },
    )
    .with_sizing_behavior(ListSizingBehavior::Infer)
    .with_width_from_item(widest_row)
    .max_h(DROPDOWN_LIST_VIEWPORT_HEIGHT)
    .track_scroll(&state.scroll);
    let options_region = div()
        .flex_none()
        .max_h(DROPDOWN_OPTIONS_MAX_HEIGHT)
        .p_1()
        .debug_selector(move || options_region_selector.clone())
        .child(option_rows.on_scroll_wheel(|_, _, cx| cx.stop_propagation()));

    deferred(
        anchored()
            .position(state.anchor)
            .snap_to_window_with_margin(px(8.))
            .child(
                div()
                    .id(format!("{id}-options"))
                    .debug_selector(move || popup_selector.clone())
                    .min_w(px(180.))
                    .max_w(px(560.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(colors.border_focused)
                    .bg(colors.elevated_surface_background)
                    .text_color(colors.text)
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .when(!query.is_empty(), |menu| {
                        menu.child(
                            div()
                                .flex_none()
                                .debug_selector(move || search_selector.clone())
                                .px_2()
                                .py_1()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child(format!("Search: {query}")),
                        )
                    })
                    .child(if no_matches {
                        div()
                            .flex_none()
                            .debug_selector(move || no_matches_selector.clone())
                            .p_1()
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .text_color(colors.text_muted)
                                    .child("No matches"),
                            )
                            .into_any_element()
                    } else {
                        options_region.into_any_element()
                    }),
            ),
    )
    .with_priority(crate::app_render::MODAL_POPUP_PAINT_PRIORITY)
    .into_any_element()
}

fn fuzzy_score(candidate: &str, query: &str) -> Option<i32> {
    let candidate = candidate.to_lowercase();
    let query = query.to_lowercase();
    if query.is_empty() {
        return Some(0);
    }

    let mut characters = query.chars();
    let mut wanted = characters.next()?;
    let mut score = 0;
    let mut previous_match = None;
    for (index, character) in candidate.char_indices() {
        if character != wanted {
            continue;
        }
        score += 10;
        if previous_match.is_some_and(|previous| previous + character.len_utf8() == index) {
            score += 8;
        }
        if index == 0
            || candidate[..index]
                .chars()
                .next_back()
                .is_some_and(|previous| matches!(previous, ' ' | ':' | '_' | '-'))
        {
            score += 5;
        }
        previous_match = Some(index);
        match characters.next() {
            Some(next) => wanted = next,
            None => return Some(score - candidate.len() as i32 / 8),
        }
    }
    None
}

pub(crate) fn fuzzy_match_index(options: &[String], query: &str) -> Option<usize> {
    if query.is_empty() {
        return (!options.is_empty()).then_some(0);
    }
    options
        .iter()
        .enumerate()
        .filter_map(|(index, option)| fuzzy_score(option, query).map(|score| (index, score)))
        .max_by(|(left_index, left_score), (right_index, right_score)| {
            left_score
                .cmp(right_score)
                .then_with(|| right_index.cmp(left_index))
        })
        .map(|(index, _)| index)
}

pub(crate) fn fuzzy_match_indices(options: &[String], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..options.len()).collect();
    }
    options
        .iter()
        .enumerate()
        .filter_map(|(index, option)| fuzzy_score(option, query).map(|_| index))
        .collect()
}

#[cfg(test)]
#[path = "tests/searchable_dropdown.rs"]
mod tests;
