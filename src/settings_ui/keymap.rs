use super::*;

#[derive(Clone, Debug)]
pub(crate) struct KeymapCapture {
    pub(crate) target: KeymapTextField,
    pub(crate) keystroke: Option<KeybindingKeystroke>,
}

pub(crate) fn is_modifier_key(key: &str) -> bool {
    matches!(
        key,
        "alt"
            | "control"
            | "ctrl"
            | "fn"
            | "function"
            | "meta"
            | "platform"
            | "shift"
            | "super"
            | "win"
            | "command"
            | "cmd"
    )
}

pub(crate) fn is_unmodified_capture_control(key: &str, modifiers: &gpui::Modifiers) -> bool {
    !modifiers.modified() && matches!(key, "escape" | "enter")
}

pub(crate) fn keybinding_for_capture(
    keystroke: &gpui::Keystroke,
    keyboard_mapper: &dyn PlatformKeyboardMapper,
) -> KeybindingKeystroke {
    KeybindingKeystroke::new_with_mapper(keystroke.clone(), false, keyboard_mapper)
}

/// A binding matches a query if its own keystroke or action name contains it, or if its
/// section's context does (so searching a context name surfaces all of that context's bindings).
pub(crate) fn keymap_search_matches(
    sections: &[KeymapSectionForm],
    query: &str,
) -> (Vec<usize>, HashMap<usize, Vec<usize>>) {
    if query.is_empty() {
        let section_indices = (0..sections.len()).collect();
        let bindings = sections
            .iter()
            .enumerate()
            .map(|(index, section)| (index, (0..section.bindings.len()).collect()))
            .collect();
        return (section_indices, bindings);
    }
    let mut filtered_sections = Vec::new();
    let mut filtered_bindings = HashMap::new();
    for (section_index, section) in sections.iter().enumerate() {
        let context_matches = section.context.text.to_lowercase().contains(query);
        let matching_bindings: Vec<usize> = section
            .bindings
            .iter()
            .enumerate()
            .filter(|(_, binding)| {
                context_matches
                    || binding.keystroke.text.to_lowercase().contains(query)
                    || binding.action_name().to_lowercase().contains(query)
            })
            .map(|(index, _)| index)
            .collect();
        if !matching_bindings.is_empty() {
            filtered_bindings.insert(section_index, matching_bindings);
            filtered_sections.push(section_index);
        }
    }
    (filtered_sections, filtered_bindings)
}

pub(crate) fn rebuild_keymap_search_cache(editor: &mut SettingsEditor) {
    let query = editor.keymap_search.text.trim().to_lowercase();
    let (sections, bindings) = keymap_search_matches(&editor.keymap.sections, &query);
    editor.keymap_search_query_cache = query;
    editor.keymap_filtered_sections = Some(sections);
    editor.keymap_filtered_bindings = bindings;
}

pub(crate) fn invalidate_keymap_cache(editor: &mut SettingsEditor) {
    editor.keymap_filtered_sections = None;
    editor.keymap_search_query_cache.clear();
    editor.keymap_filtered_bindings.clear();
}

/// Returns the current search-filtered (section, bindings) indices, using the cache
/// when it's still valid for the current query and recomputing inline otherwise
/// (render only has `&SettingsEditor`, so it can't refresh the cache in place).
pub(crate) fn keymap_filtered_indices(
    editor: &SettingsEditor,
) -> (Vec<usize>, HashMap<usize, Vec<usize>>) {
    let query = editor.keymap_search.text.trim().to_lowercase();
    if editor.keymap_search_query_cache == query
        && let Some(sections) = editor.keymap_filtered_sections.as_ref()
    {
        return (sections.clone(), editor.keymap_filtered_bindings.clone());
    }
    keymap_search_matches(&editor.keymap.sections, &query)
}

/// A single row of the virtualized keymap list, in display order. Kept in sync with
/// [`build_settings_controls`]'s `SettingsPage::Keymap` arm, which walks the same
/// filtered indices, so keyboard navigation and rendering never disagree about
/// which bindings are visible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeymapRow {
    SectionHeader(usize),
    Binding(usize, usize),
    AddBinding(usize),
    AddSection,
}

pub(crate) fn keymap_rows(editor: &SettingsEditor) -> Vec<KeymapRow> {
    let (filtered_sections, filtered_bindings) = keymap_filtered_indices(editor);
    let mut rows = Vec::new();
    for section_index in filtered_sections {
        rows.push(KeymapRow::SectionHeader(section_index));
        if let Some(binding_indices) = filtered_bindings.get(&section_index) {
            rows.extend(
                binding_indices
                    .iter()
                    .map(|&binding_index| KeymapRow::Binding(section_index, binding_index)),
            );
        }
        rows.push(KeymapRow::AddBinding(section_index));
    }
    rows.push(KeymapRow::AddSection);
    rows
}

impl Zetta {
    pub(crate) fn start_keymap_capture(
        &mut self,
        target: KeymapTextField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        if editor.keymap.text_mut(target).is_none() {
            return;
        }
        editor.keymap_capture = Some(KeymapCapture {
            target,
            keystroke: None,
        });
        editor.focused_input = None;
        editor.focused_control = Some(SettingsControl::CaptureKeymap(target));
        editor.open_dropdown = None;
        editor.dropdown_query.clear();
        editor.message = None;
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn cancel_keymap_capture(
        &mut self,
        target: KeymapTextField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        if editor
            .keymap_capture
            .as_ref()
            .is_some_and(|capture| capture.target == target)
        {
            editor.keymap_capture = None;
            self.focus_settings_input(SettingsInput::Keymap(target), window, cx);
        }
    }

    pub(crate) fn commit_keymap_capture(
        &mut self,
        target: KeymapTextField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        let Some(capture) = editor.keymap_capture.take() else {
            return;
        };
        if capture.target != target {
            editor.keymap_capture = Some(capture);
            return;
        }
        let Some(keystroke) = capture.keystroke else {
            editor.keymap_capture = Some(capture);
            return;
        };
        let text = keymap_keystroke_display(&keystroke.unparse());
        if let Some(field) = editor.keymap.text_mut(target) {
            field.text = text;
            field.cursor = field.text.len();
            field.select_all = false;
            editor.keymap_dirty = true;
            editor.message = None;
        }
        self.focus_settings_input(SettingsInput::Keymap(target), window, cx);
    }
}

#[cfg(test)]
#[path = "../tests/settings_ui/keymap.rs"]
mod tests;
