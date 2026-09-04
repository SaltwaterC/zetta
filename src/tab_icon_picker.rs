use std::sync::{Arc, OnceLock};

use strum::IntoEnumIterator as _;

use crate::TextField;
use crate::project_form::ProjectTabIcon;
use crate::settings_ui::projects::mark_project_dirty;
use gpui::{ScrollStrategy, SharedString, UniformListScrollHandle};
use ui::IconName;

use super::*;
use gpui::{ListSizingBehavior, uniform_list};

/// An icon paired with cached display and search labels, so filtering and
/// rendering never have to re-run `format!("{icon:?}")` on the UI thread.
pub(crate) struct IconEntry {
    pub(crate) icon: IconName,
    pub(crate) label: SharedString,
    pub(crate) search_label: String,
}

pub(crate) const TAB_ICON_COLUMNS: usize = 7;

/// The icon grid's cell geometry. Shared by the picker, which sizes its
/// viewport from it, and by the grid itself.
const ICON_CELL_WIDTH: Pixels = px(84.);
const ICON_CELL_HEIGHT: Pixels = px(68.);
const ICON_CELL_PADDING: Pixels = px(1.);
const ICON_GAP: Pixels = px(1.);

pub(crate) const fn tab_icon_row(index: usize) -> usize {
    index / TAB_ICON_COLUMNS
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabIconPickerTarget {
    Tab(usize),
    Default,
    /// The `default_tab_icon` of the project the Projects tab is building.
    ProjectDefault,
}

pub(crate) struct TabIconPicker {
    pub(crate) target: TabIconPickerTarget,
    pub(crate) query: TextField,
    pub(crate) selected: usize,
    pub(crate) scroll: UniformListScrollHandle,
    entries: Arc<[IconEntry]>,
    options_cache: Arc<[Option<usize>]>,
    options_query_cache: String,
}

impl TabIconPicker {
    pub(crate) fn new(
        target: TabIconPickerTarget,
        current_icon: Option<IconName>,
        entries: Arc<[IconEntry]>,
    ) -> Self {
        let options = filter_icon_entries(&entries, "");
        let selected = options
            .iter()
            .position(|option| icon_for_option(&entries, *option) == current_icon)
            .unwrap_or_default();
        Self {
            target,
            query: TextField::default(),
            selected,
            scroll: UniformListScrollHandle::new(),
            entries,
            options_cache: options.into(),
            options_query_cache: String::new(),
        }
    }

    /// The icon options matching the picker's current search query, shared
    /// by keyboard handling and rendering so both see identical results and
    /// neither redoes the filter when the query hasn't changed since the
    /// last call.
    pub(crate) fn options(&mut self) -> Arc<[Option<usize>]> {
        if self.options_query_cache != self.query.text {
            self.options_cache = filter_icon_entries(&self.entries, &self.query.text).into();
            self.options_query_cache = self.query.text.clone();
        }
        self.options_cache.clone()
    }

    pub(crate) fn icon_for_option(&self, option: Option<usize>) -> Option<IconName> {
        icon_for_option(&self.entries, option)
    }

    pub(crate) fn entries(&self) -> Arc<[IconEntry]> {
        self.entries.clone()
    }
}

pub(crate) fn tab_icon_label(icon: IconName) -> String {
    format!("{icon:?}")
}

pub(crate) fn build_icon_entries(icons: &[IconName]) -> Vec<IconEntry> {
    icons
        .iter()
        .map(|icon| {
            let label = tab_icon_label(*icon);
            IconEntry {
                icon: *icon,
                label: label.clone().into(),
                search_label: label.to_lowercase(),
            }
        })
        .collect()
}

/// Fallback icon entries for use before the background-populated icon
/// cache on `Zetta` is ready. Computed at most once per process.
pub(crate) fn fallback_icon_entries() -> Arc<[IconEntry]> {
    static FALLBACK: OnceLock<Arc<[IconEntry]>> = OnceLock::new();
    FALLBACK
        .get_or_init(|| build_icon_entries(&IconName::iter().collect::<Vec<_>>()).into())
        .clone()
}

fn icon_for_option(entries: &[IconEntry], option: Option<usize>) -> Option<IconName> {
    option.and_then(|index| entries.get(index).map(|entry| entry.icon))
}

fn filter_icon_entries(entries: &[IconEntry], query: &str) -> Vec<Option<usize>> {
    let query = query.to_lowercase();
    let mut options = Vec::new();
    if query.is_empty() || "none".contains(&query) || "no icon".contains(&query) {
        options.push(None);
    }
    options.extend(
        entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.search_label.contains(&query))
            .map(|(index, _)| Some(index)),
    );
    options
}

/// Convenience wrapper over the fallback (uncached) icon entries; only
/// used by tests, which exercise filtering without going through a
/// `TabIconPicker`/`Zetta` instance.
#[cfg(test)]
pub(crate) fn matching_tab_icons(query: &str) -> Vec<IconName> {
    let entries = fallback_icon_entries();
    filter_icon_entries(&entries, query)
        .into_iter()
        .filter_map(|option| icon_for_option(&entries, option))
        .collect()
}

/// Convenience wrapper over the fallback (uncached) icon entries; only
/// used by tests, which exercise filtering without going through a
/// `TabIconPicker`/`Zetta` instance.
#[cfg(test)]
pub(crate) fn matching_tab_icon_options(query: &str) -> Vec<Option<IconName>> {
    let entries = fallback_icon_entries();
    filter_icon_entries(&entries, query)
        .into_iter()
        .map(|option| icon_for_option(&entries, option))
        .collect()
}

pub(crate) fn tab_icon_completion_names() -> impl Iterator<Item = &'static str> {
    std::iter::once("none").chain(IconName::iter().map(|icon| {
        let name: &'static str = icon.into();
        name
    }))
}

pub(crate) fn parse_tab_icon_name(name: &str) -> Option<IconName> {
    IconName::iter().find(|icon| {
        let cli_name: &'static str = (*icon).into();
        cli_name.eq_ignore_ascii_case(name) || tab_icon_label(*icon).eq_ignore_ascii_case(name)
    })
}

impl Zetta {
    pub(crate) fn change_tab_icon(
        &mut self,
        _: &ChangeTabIcon,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_tab_icon_picker(self.active_tab, window, cx);
    }

    pub(crate) fn set_active_tab_icon_from_cli(
        &mut self,
        icon: Option<IconName>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return false;
        };
        tab.set_icon_override(icon);
        cx.notify();
        true
    }

    /// Shared icon entries (icon + precomputed lowercase label) backing the
    /// tab icon picker, whether opened for a specific tab or for the
    /// config default icon. Reads the background-populated cache, falling
    /// back to a lazily-computed set if it isn't ready yet.
    pub(crate) fn tab_icon_entries(&self) -> Arc<[IconEntry]> {
        self.icon_cache
            .get()
            .map_or_else(fallback_icon_entries, |cache| cache.entries.clone())
    }

    pub(crate) fn open_tab_icon_picker(
        &mut self,
        tab_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if tab_index >= self.tabs.len() {
            return;
        }
        let current_icon = self.tabs.get(tab_index).and_then(|tab| tab.icon);
        let entries = self.tab_icon_entries();
        self.tab_icon_picker = Some(TabIconPicker::new(
            TabIconPickerTarget::Tab(tab_index),
            current_icon,
            entries,
        ));
        if let Some(picker) = self.tab_icon_picker.as_ref() {
            picker
                .scroll
                .scroll_to_item(tab_icon_row(picker.selected), ScrollStrategy::Nearest);
        }
        self.tab_icon_picker_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn open_default_tab_icon_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_editor.is_none() {
            return;
        }
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.focused_control = Some(SettingsControl::DefaultTabIconPicker);
            editor.focused_input = None;
        }
        let current_icon = self
            .settings_editor
            .as_ref()
            .and_then(|editor| editor.configuration.default_tab_icon);
        let entries = self.tab_icon_entries();
        self.tab_icon_picker = Some(TabIconPicker::new(
            TabIconPickerTarget::Default,
            current_icon,
            entries,
        ));
        if let Some(picker) = self.tab_icon_picker.as_ref() {
            picker
                .scroll
                .scroll_to_item(tab_icon_row(picker.selected), ScrollStrategy::Nearest);
        }
        self.tab_icon_picker_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn open_project_tab_icon_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(current_icon) = self
            .settings_editor
            .as_ref()
            .and_then(crate::settings_ui::project_editor)
            .map(|project| project.form.default_tab_icon.icon())
        else {
            return;
        };
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.focused_control = Some(SettingsControl::ProjectTabIconPicker);
            editor.focused_input = None;
        }
        let entries = self.tab_icon_entries();
        self.tab_icon_picker = Some(TabIconPicker::new(
            TabIconPickerTarget::ProjectDefault,
            current_icon,
            entries,
        ));
        if let Some(picker) = self.tab_icon_picker.as_ref() {
            picker
                .scroll
                .scroll_to_item(tab_icon_row(picker.selected), ScrollStrategy::Nearest);
        }
        self.tab_icon_picker_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn dismiss_tab_icon_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.tab_icon_picker.take().map(|picker| picker.target);
        if matches!(
            target,
            Some(TabIconPickerTarget::Default | TabIconPickerTarget::ProjectDefault)
        ) {
            self.settings_focus.focus(window, cx);
        } else {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    pub(crate) fn set_tab_icon(
        &mut self,
        icon: Option<IconName>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self.tab_icon_picker.as_ref() else {
            return;
        };
        match picker.target {
            TabIconPickerTarget::Tab(tab_index) => {
                if let Some(tab) = self.tabs.get_mut(tab_index) {
                    tab.set_icon_override(icon);
                }
            }
            TabIconPickerTarget::Default => {
                if let Some(editor) = self.settings_editor.as_mut() {
                    editor.configuration.default_tab_icon = icon;
                    editor.configuration_dirty = true;
                    editor.message = None;
                }
            }
            TabIconPickerTarget::ProjectDefault => {
                if let Some(editor) = self.settings_editor.as_mut() {
                    if let Some(project) = editor.project.as_mut() {
                        // The picker's "None" option is an explicit `null` in a
                        // project file, which is how a project turns off an icon
                        // the user configuration sets; clearing the override back
                        // to inherited is a separate control.
                        project.form.default_tab_icon = match icon {
                            Some(icon) => ProjectTabIcon::Icon(icon),
                            None => ProjectTabIcon::None,
                        };
                    }
                    mark_project_dirty(editor);
                }
            }
        }
        self.dismiss_tab_icon_picker(window, cx);
    }

    pub(crate) fn tab_icon_picker_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
        if self.tab_icon_picker.is_none() {
            return;
        }
        cx.stop_propagation();
        // Settled before this picker's own keys, so `Ctrl-X` cuts rather than
        // typing an `x` and `Shift-Delete` cuts rather than forward-deleting.
        if let Some(picker) = self.tab_icon_picker.as_mut() {
            match apply_clipboard_shortcut(&mut picker.query, &event.keystroke, cx) {
                ClipboardOutcome::Ignored => {}
                ClipboardOutcome::Unchanged => {
                    cx.notify();
                    return;
                }
                ClipboardOutcome::Edited => {
                    // The query filters the icon grid, so the selection returns
                    // to the first match rather than pointing into the old one.
                    picker.selected = 0;
                    picker
                        .scroll
                        .scroll_to_item(tab_icon_row(picker.selected), ScrollStrategy::Nearest);
                    cx.notify();
                    return;
                }
            }
        }
        if event.keystroke.key == "escape" {
            self.dismiss_tab_icon_picker(window, cx);
            return;
        }
        let activate = {
            let mut activate = None;
            let mut query_changed = false;
            let mut selection_changed = false;
            let Some(picker) = self.tab_icon_picker.as_mut() else {
                return;
            };
            let options = picker.options();
            match event.keystroke.key.as_str() {
                "up" if !command => {
                    picker.selected = picker.selected.saturating_sub(TAB_ICON_COLUMNS);
                    selection_changed = true;
                }
                "down" if !command => {
                    picker.selected =
                        (picker.selected + TAB_ICON_COLUMNS).min(options.len().saturating_sub(1));
                    selection_changed = true;
                }
                "tab" if !command => {
                    if event.keystroke.modifiers.shift {
                        picker.selected = picker.selected.saturating_sub(1);
                    } else {
                        picker.selected =
                            (picker.selected + 1).min(options.len().saturating_sub(1));
                    }
                    selection_changed = true;
                }
                "enter" if !command => {
                    activate = options
                        .get(picker.selected)
                        .copied()
                        .map(|option| picker.icon_for_option(option));
                }
                _ => match apply_text_field_key(&mut picker.query, &event.keystroke) {
                    // The grid's own keys are settled above, so anything left
                    // that the field does not answer to is not for this picker.
                    TextFieldEdit::Ignored => return,
                    TextFieldEdit::CursorMoved => {}
                    TextFieldEdit::Edited => query_changed = true,
                },
            }
            if query_changed {
                picker.selected = 0;
                selection_changed = true;
            }
            if selection_changed {
                picker
                    .scroll
                    .scroll_to_item(tab_icon_row(picker.selected), ScrollStrategy::Nearest);
            }
            activate
        };
        if let Some(icon) = activate {
            self.set_tab_icon(icon, window, cx);
        } else {
            cx.notify();
        }
    }
}

#[cfg(test)]
#[path = "tests/tab_icon_picker.rs"]
mod tests;

impl Zetta {
    pub(crate) fn render_tab_icon_picker_overlay(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        self.tab_icon_picker.as_ref()?;

        // Get options and picker state, then release the borrow
        let (options, picker_state) = {
            let picker = self.tab_icon_picker.as_mut().expect("checked above");
            let opts = picker.options();
            let state = (
                picker.target,
                picker.selected,
                picker.scroll.clone(),
                picker.entries(),
            );
            (opts, state)
        };

        let (target, selected_index, scroll_handle, entries) = picker_state;
        let colors = self.window_theme(cx).colors().clone();
        let handle = cx.entity().downgrade();

        // Get selected icon for highlighting
        let selected_icon = match target {
            TabIconPickerTarget::Tab(tab_index) => {
                self.tabs.get(tab_index).and_then(|tab| tab.icon)
            }
            TabIconPickerTarget::Default => self
                .settings_editor
                .as_ref()
                .and_then(|editor| editor.configuration.default_tab_icon),
            TabIconPickerTarget::ProjectDefault => self
                .settings_editor
                .as_ref()
                .and_then(crate::settings_ui::project_editor)
                .and_then(|project| project.form.default_tab_icon.icon()),
        };

        let picker = self.tab_icon_picker.as_ref()?;
        let query = picker.query.clone();

        let row_count = options.len().div_ceil(TAB_ICON_COLUMNS);

        // Build search bar
        let search_handle = handle.clone();
        // The picker has one field and it always holds the focus while the
        // picker is open, so the frame is always the focused one.
        let search = field_box("tab-icon-search", true, &colors)
            .min_w_0()
            .flex_1()
            .when(query.select_all, |input| {
                input.bg(colors.element_selection_background)
            })
            .text_color(colors.text)
            .child(field_query_run(&query, Some("Search icons…"), &colors))
            .on_click(move |_, window, cx| {
                search_handle
                    .update(cx, |this, cx| {
                        this.tab_icon_picker_focus.focus(window, cx);
                    })
                    .ok();
            });

        let close_handle = handle.clone();
        let close = div()
            .id("close-tab-icon-picker")
            .flex_none()
            .px_3()
            .py_1()
            .rounded(px(4.))
            .border_1()
            .border_color(colors.element_selected)
            .cursor_pointer()
            .bg(colors.element_selected)
            .text_color(colors.text)
            .hover(|style| style.bg(colors.element_hover))
            .tooltip(Tooltip::text("Close tab icon picker (Esc)"))
            .on_click(move |_, window, cx| {
                close_handle
                    .update(cx, |this, cx| this.dismiss_tab_icon_picker(window, cx))
                    .ok();
            })
            .child("Close");

        // Virtualized icon grid using uniform_list
        let icon_rows = tab_icon_grid(TabIconGrid {
            options: options.clone(),
            entries,
            selected_icon,
            selected_index,
            row_count,
            colors: colors.clone(),
            handle: handle.clone(),
            scroll_handle: scroll_handle.clone(),
        });

        let has_options = !options.is_empty();

        Some(
            div()
                .id("tab-icon-picker-modal")
                .absolute()
                .inset_0()
                .p_8()
                .flex()
                .items_center()
                .justify_center()
                .bg(transparent_black().opacity(0.55))
                .occlude()
                .child(
                    div()
                        .w_full()
                        .max_w(px(720.))
                        .h_full()
                        .max_h(px(600.))
                        .p_3()
                        .flex()
                        .flex_col()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .text_color(colors.text)
                        .shadow_lg()
                        .child(h_flex().mb_3().gap_2().child(search).child(close))
                        .child(
                            div()
                                .relative()
                                .min_h_0()
                                .flex_1()
                                .overflow_hidden()
                                .when(has_options, |container| container.child(icon_rows))
                                .when(!has_options, |container| {
                                    container.child(
                                        div()
                                            .w_full()
                                            .py_6()
                                            .flex()
                                            .justify_center()
                                            .text_color(colors.text_muted)
                                            .child("No icons match your search"),
                                    )
                                })
                        )
                        .child(
                            h_flex()
                                .mt_2()
                                .w_full()
                                .justify_center()
                                .text_color(colors.text_muted)
                                .text_xs()
                                .child("Tab / Shift-Tab: navigate icons  •  ↑/↓: navigate rows  •  ←/→: move cursor in search  •  Enter: select  •  Esc: close"),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// What the virtualized icon grid needs. Owned rather than borrowed because the
/// `uniform_list` batch closure outlives this frame's borrows.
struct TabIconGrid {
    options: Arc<[Option<usize>]>,
    entries: Arc<[IconEntry]>,
    selected_icon: Option<IconName>,
    selected_index: usize,
    row_count: usize,
    colors: ThemeColors,
    handle: WeakEntity<Zetta>,
    scroll_handle: gpui::UniformListScrollHandle,
}

/// The icon grid: `TAB_ICON_COLUMNS` cells per row, virtualized so only the
/// visible rows are built however many icons there are.
fn tab_icon_grid(grid: TabIconGrid) -> gpui::UniformList {
    let TabIconGrid {
        options,
        entries,
        selected_icon,
        selected_index,
        row_count,
        colors,
        handle,
        scroll_handle,
    } = grid;
    let row_height = ICON_CELL_HEIGHT + ICON_CELL_PADDING * 2. + ICON_GAP;
    let row_colors = colors;
    let row_entries = entries.clone();
    let row_handle = handle.clone();
    let row_selected_icon = selected_icon;
    let row_selected_index = selected_index;
    let options_for_rows = options.clone();

    uniform_list(
        "tab-icon-grid",
        row_count,
        move |range: std::ops::Range<usize>, _, _| {
            let entries = row_entries.clone();
            let options = &options_for_rows;
            let row_handle = row_handle.clone();
            // Borrowed, not cloned: the cells only read `Hsla` fields out of
            // it, and this runs once per batch of visible rows per scroll
            // frame — `ThemeColors` is 143 fields.
            let row_colors = &row_colors;
            let row_selected_icon = row_selected_icon;
            let row_selected_index = row_selected_index;

            range
                .map(|row_index| {
                    let row_start = row_index * TAB_ICON_COLUMNS;
                    let row_end = (row_start + TAB_ICON_COLUMNS).min(options.len());
                    let row_options = &options[row_start..row_end];

                    let cells: Vec<AnyElement> = row_options
                        .iter()
                        .enumerate()
                        .map(|(col_index, option)| {
                            // Copy the option value since IconName is Copy
                            let option = *option;
                            let index = row_start + col_index;
                            let icon =
                                option.and_then(|index| entries.get(index).map(|entry| entry.icon));
                            let label = option
                                .and_then(|index| {
                                    entries.get(index).map(|entry| entry.label.clone())
                                })
                                .unwrap_or_else(|| SharedString::new_static("None"));
                            let keyboard_selected = index == row_selected_index;
                            let icon_for_click = icon;
                            let icon_handle = row_handle.clone();

                            div()
                                .id(("tab-icon-option", index))
                                .w(ICON_CELL_WIDTH)
                                .h(ICON_CELL_HEIGHT)
                                .p_1()
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap_1()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .when(icon == row_selected_icon, |cell| {
                                    cell.bg(row_colors.element_selected)
                                })
                                .when(keyboard_selected, |cell| {
                                    cell.border_1().border_color(row_colors.border_focused)
                                })
                                .hover(|cell| cell.bg(row_colors.element_hover))
                                .when_some(icon, |cell, icon| {
                                    cell.child(
                                        Icon::new(icon)
                                            .size(IconSize::Medium)
                                            .color(Color::Custom(row_colors.icon)),
                                    )
                                })
                                .when(icon.is_none(), |cell| {
                                    cell.child(
                                        Icon::new(IconName::Dash)
                                            .size(IconSize::Medium)
                                            .color(Color::Custom(row_colors.icon)),
                                    )
                                })
                                .child(
                                    Label::new(label.clone())
                                        .size(LabelSize::XSmall)
                                        .color(Color::Custom(row_colors.text))
                                        .truncate(),
                                )
                                .tooltip(Tooltip::text(label))
                                .on_click(move |_, window, cx| {
                                    icon_handle
                                        .update(cx, |this, cx| {
                                            this.set_tab_icon(icon_for_click, window, cx);
                                        })
                                        .ok();
                                })
                                .into_any_element()
                        })
                        .collect();

                    div()
                        .h(row_height)
                        .flex()
                        .gap_1()
                        .children(cells)
                        .into_any_element()
                })
                .collect()
        },
    )
    .with_sizing_behavior(ListSizingBehavior::Infer)
    .w_full()
    .h_full()
    .track_scroll(&scroll_handle)
    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
}
