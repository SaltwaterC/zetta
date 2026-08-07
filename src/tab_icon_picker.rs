use std::sync::{Arc, OnceLock};

use strum::IntoEnumIterator as _;

use crate::TextField;
use gpui::ScrollHandle;
use ui::IconName;

use super::*;

/// An icon paired with its precomputed, lowercased search label, so
/// filtering never has to re-run `format!("{icon:?}")` per keystroke.
pub(crate) type IconEntry = (IconName, String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabIconPickerTarget {
    Tab(usize),
    Default,
}

pub(crate) struct TabIconPicker {
    pub(crate) target: TabIconPickerTarget,
    pub(crate) query: TextField,
    pub(crate) selected: usize,
    pub(crate) scroll: ScrollHandle,
    options_cache: Vec<Option<IconName>>,
    options_query_cache: String,
}

impl TabIconPicker {
    pub(crate) fn new(
        target: TabIconPickerTarget,
        current_icon: Option<IconName>,
        entries: &[IconEntry],
    ) -> Self {
        let options = filter_icon_entries(entries, "");
        let selected = options
            .iter()
            .position(|icon| *icon == current_icon)
            .unwrap_or_default();
        Self {
            target,
            query: TextField::default(),
            selected,
            scroll: ScrollHandle::new(),
            options_cache: options,
            options_query_cache: String::new(),
        }
    }

    /// The icon options matching the picker's current search query, shared
    /// by keyboard handling and rendering so both see identical results and
    /// neither redoes the filter when the query hasn't changed since the
    /// last call.
    pub(crate) fn options(&mut self, entries: &[IconEntry]) -> &[Option<IconName>] {
        if self.options_query_cache != self.query.text {
            self.options_cache = filter_icon_entries(entries, &self.query.text);
            self.options_query_cache = self.query.text.clone();
        }
        &self.options_cache
    }
}

pub(crate) fn tab_icon_label(icon: IconName) -> String {
    format!("{icon:?}")
}

pub(crate) fn build_icon_entries(icons: &[IconName]) -> Vec<IconEntry> {
    icons
        .iter()
        .map(|icon| (*icon, tab_icon_label(*icon).to_lowercase()))
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

fn filter_icon_entries(entries: &[IconEntry], query: &str) -> Vec<Option<IconName>> {
    let query = query.to_lowercase();
    let mut options = Vec::new();
    if query.is_empty() || "none".contains(&query) || "no icon".contains(&query) {
        options.push(None);
    }
    options.extend(
        entries
            .iter()
            .filter(|(_, label)| label.contains(&query))
            .map(|(icon, _)| Some(*icon)),
    );
    options
}

/// Convenience wrapper over the fallback (uncached) icon entries; only
/// used by tests, which exercise filtering without going through a
/// `TabIconPicker`/`Zetta` instance.
#[cfg(test)]
pub(crate) fn matching_tab_icons(query: &str) -> Vec<IconName> {
    filter_icon_entries(&fallback_icon_entries(), query)
        .into_iter()
        .flatten()
        .collect()
}

/// Convenience wrapper over the fallback (uncached) icon entries; only
/// used by tests, which exercise filtering without going through a
/// `TabIconPicker`/`Zetta` instance.
#[cfg(test)]
pub(crate) fn matching_tab_icon_options(query: &str) -> Vec<Option<IconName>> {
    filter_icon_entries(&fallback_icon_entries(), query)
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
        tab.icon = icon;
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
            .map(|cache| cache.entries.clone())
            .unwrap_or_else(fallback_icon_entries)
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
            &entries,
        ));
        if let Some(picker) = self.tab_icon_picker.as_ref() {
            picker.scroll.scroll_to_item(picker.selected);
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
            &entries,
        ));
        if let Some(picker) = self.tab_icon_picker.as_ref() {
            picker.scroll.scroll_to_item(picker.selected);
        }
        self.tab_icon_picker_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn dismiss_tab_icon_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.tab_icon_picker.take().map(|picker| picker.target);
        if target == Some(TabIconPickerTarget::Default) {
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
                    tab.icon = icon;
                }
            }
            TabIconPickerTarget::Default => {
                if let Some(editor) = self.settings_editor.as_mut() {
                    editor.configuration.default_tab_icon = icon;
                    editor.configuration_dirty = true;
                    editor.message = None;
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
        if event.keystroke.key == "escape" {
            self.dismiss_tab_icon_picker(window, cx);
            return;
        }
        let entries = self.tab_icon_entries();
        let activate = {
            let mut activate = None;
            let mut query_changed = false;
            let mut selection_changed = false;
            let Some(picker) = self.tab_icon_picker.as_mut() else {
                return;
            };
            let options = picker.options(&entries).to_vec();
            match event.keystroke.key.as_str() {
                "left" if !command => picker.query.move_left(),
                "right" if !command => picker.query.move_right(),
                "up" if !command => {
                    picker.selected = picker.selected.saturating_sub(7);
                    selection_changed = true;
                }
                "down" if !command => {
                    picker.selected = (picker.selected + 7).min(options.len().saturating_sub(1));
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
                    activate = options.get(picker.selected).copied();
                }
                "backspace" => {
                    picker.query.backspace();
                    query_changed = true;
                }
                "delete" => {
                    picker.query.delete();
                    query_changed = true;
                }
                "home" => {
                    picker.query.cursor = 0;
                    picker.query.select_all = false;
                }
                "end" => {
                    picker.query.cursor = picker.query.text.len();
                    picker.query.select_all = false;
                }
                "a" if command => picker.query.select_all(),
                _ if !command
                    && !event.keystroke.modifiers.alt
                    && event.keystroke.key_char.is_some() =>
                {
                    if let Some(text) = event.keystroke.key_char.as_ref() {
                        picker.query.insert(text);
                        query_changed = true;
                    }
                }
                _ => return,
            }
            if query_changed {
                picker.selected = 0;
                selection_changed = true;
            }
            if selection_changed {
                picker.scroll.scroll_to_item(picker.selected);
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
