use super::*;
use crate::settings_view::KEYMAP_ROW_HEIGHT;
use smallvec::SmallVec;
use ui::StickyCandidate;
use ui::{Button, ButtonStyle, div, h_flex, px};

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

pub(crate) const GLOBAL_CONTEXT_LABEL: &str = "Global";

pub(crate) fn keymap_context_label(context: &str) -> &str {
    if context.is_empty() {
        GLOBAL_CONTEXT_LABEL
    } else {
        context
    }
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
        let context_matches = keymap_context_label(&section.context.text)
            .to_lowercase()
            .contains(query);
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

/// Recomputes every derived view of the keymap: the search-filtered indices, the
/// row list, and the per-row render data. Called from each site that edits the
/// keymap form or its search query, so rendering only ever clones `Arc`s — the
/// keymap page renders this model twice per frame (list plus sticky headers) and
/// rebuilding it there costs a `is_default_binding` lookup per row per frame.
pub(crate) fn refresh_keymap_cache(editor: &mut SettingsEditor) {
    let query = editor.keymap_search.text.trim().to_lowercase();
    let (sections, bindings) = keymap_search_matches(&editor.keymap.sections, &query);
    editor.keymap_search_query_cache = query;
    editor.keymap_filtered_sections = Some(sections);
    editor.keymap_filtered_bindings = bindings;
    let rows = build_keymap_rows(editor);
    editor.keymap_row_data_cache = Some(build_keymap_row_data(editor, &rows).into());
    editor.keymap_rows_cache = Some(rows.into());
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
    UnboundDefault(usize, usize),
    AddBinding(usize),
    AddSection,
}

/// The rows the keymap list displays, from the cache when it is still valid for
/// the current search query and recomputed inline otherwise (render only has
/// `&SettingsEditor`, so it cannot refresh the cache in place).
pub(crate) fn keymap_rows(editor: &SettingsEditor) -> Arc<[KeymapRow]> {
    if keymap_cache_is_current(editor)
        && let Some(rows) = editor.keymap_rows_cache.as_ref()
    {
        return rows.clone();
    }
    build_keymap_rows(editor).into()
}

/// The per-row render data for [`keymap_rows`], with the same cache contract.
pub(crate) fn keymap_row_data(editor: &SettingsEditor) -> Arc<[KeymapRowData]> {
    if keymap_cache_is_current(editor)
        && let Some(row_data) = editor.keymap_row_data_cache.as_ref()
    {
        return row_data.clone();
    }
    build_keymap_row_data(editor, &keymap_rows(editor)).into()
}

fn keymap_cache_is_current(editor: &SettingsEditor) -> bool {
    editor.keymap_search_query_cache == editor.keymap_search.text.trim().to_lowercase()
}

fn build_keymap_rows(editor: &SettingsEditor) -> Vec<KeymapRow> {
    let (filtered_sections, filtered_bindings) = keymap_filtered_indices(editor);
    keymap_rows_from_matches(
        &editor.keymap.sections,
        &filtered_sections,
        &filtered_bindings,
    )
}

pub(crate) fn keymap_rows_from_matches(
    sections: &[KeymapSectionForm],
    filtered_sections: &[usize],
    filtered_bindings: &HashMap<usize, Vec<usize>>,
) -> Vec<KeymapRow> {
    let mut rows = Vec::new();
    for &section_index in filtered_sections {
        rows.push(KeymapRow::SectionHeader(section_index));
        if let Some(binding_indices) = filtered_bindings.get(&section_index) {
            rows.extend(
                binding_indices
                    .iter()
                    .map(|&binding_index| KeymapRow::Binding(section_index, binding_index)),
            );
        }
        // Add unbound default bindings for this section
        if let Some(section) = sections.get(section_index) {
            for (unbound_index, _) in section.unbound_defaults.iter().enumerate() {
                rows.push(KeymapRow::UnboundDefault(section_index, unbound_index));
            }
        }
        rows.push(KeymapRow::AddBinding(section_index));
    }
    rows.push(KeymapRow::AddSection);
    rows
}

/// Owned per-row data for the virtualized keymap list, extracted from
/// `SettingsEditor` because the list's row closure must be `'static` and so
/// cannot hold a borrow of it.
pub(crate) enum KeymapRowData {
    SectionHeader {
        section_index: usize,
        context: TextField,
    },
    Binding {
        section_index: usize,
        binding_index: usize,
        keystroke: TextField,
        action_name: String,
        template_name: Option<String>,
        profile_name: Option<String>,
        is_default: bool,
    },
    UnboundDefault {
        section_index: usize,
        binding_index: usize,
        keystroke: TextField,
        action_name: String,
    },
    AddBinding {
        section_index: usize,
        context: String,
    },
    AddSection,
}

fn build_keymap_row_data(editor: &SettingsEditor, rows: &[KeymapRow]) -> Vec<KeymapRowData> {
    rows.iter()
        .filter_map(|row| match *row {
            KeymapRow::SectionHeader(section_index) => {
                let section = editor.keymap.sections.get(section_index)?;
                Some(KeymapRowData::SectionHeader {
                    section_index,
                    context: section.context.clone(),
                })
            }
            KeymapRow::Binding(section_index, binding_index) => {
                let binding = editor
                    .keymap
                    .sections
                    .get(section_index)?
                    .bindings
                    .get(binding_index)?;
                let profile_name = binding.action_usize_parameter("slot").map(|slot| {
                    editor
                        .profile_names
                        .get(slot.saturating_sub(1))
                        .cloned()
                        .unwrap_or_else(|| format!("Profile {slot}"))
                });
                let is_default = editor.is_default_binding(section_index, binding_index);
                Some(KeymapRowData::Binding {
                    section_index,
                    binding_index,
                    keystroke: binding.keystroke.clone(),
                    action_name: binding.action_name(),
                    template_name: binding.action_parameter("name"),
                    profile_name,
                    is_default,
                })
            }
            KeymapRow::AddBinding(section_index) => {
                let context = editor
                    .keymap
                    .sections
                    .get(section_index)
                    .map(|section| keymap_context_label(&section.context.text).to_owned())
                    .unwrap_or_default();
                Some(KeymapRowData::AddBinding {
                    section_index,
                    context,
                })
            }
            KeymapRow::UnboundDefault(section_index, binding_index) => {
                let binding = editor
                    .keymap
                    .sections
                    .get(section_index)?
                    .unbound_defaults
                    .get(binding_index)?;
                Some(KeymapRowData::UnboundDefault {
                    section_index,
                    binding_index,
                    keystroke: binding.keystroke.clone(),
                    action_name: binding.action_name(),
                })
            }
            KeymapRow::AddSection => Some(KeymapRowData::AddSection),
        })
        .collect()
}

/// A candidate for sticky section headers in the keymap list.
/// Section headers have depth 0, all other rows have depth 1.
#[derive(Clone, Debug)]
pub(crate) struct KeymapStickyCandidate {
    pub(crate) row: KeymapRow,
    pub(crate) depth: usize,
}

impl StickyCandidate for KeymapStickyCandidate {
    fn depth(&self) -> usize {
        self.depth
    }
}

/// Compute sticky candidates for a range of keymap rows.
/// This is called with &mut Zetta, so we access the settings_editor from there.
pub(crate) fn compute_keymap_sticky_candidates(
    zetta: &mut Zetta,
    range: std::ops::Range<usize>,
    _window: &mut gpui::Window,
    _cx: &mut gpui::Context<Zetta>,
) -> SmallVec<[KeymapStickyCandidate; 8]> {
    let Some(editor) = zetta.settings_editor.as_mut() else {
        return SmallVec::new();
    };
    let rows = keymap_rows(editor);
    let range_end = range.end.min(rows.len());
    let mut candidates = SmallVec::new();
    for row in rows.iter().take(range_end).skip(range.start) {
        let depth = match row {
            KeymapRow::SectionHeader(_) => 0,
            KeymapRow::AddSection => 0,
            _ => 1,
        };
        candidates.push(KeymapStickyCandidate { row: *row, depth });
    }
    candidates
}

/// Render a sticky candidate as a section header.
/// This is called with &mut Zetta, so we access the settings_editor and theme from there.
pub(crate) fn render_keymap_sticky_candidate(
    zetta: &mut Zetta,
    candidate: KeymapStickyCandidate,
    _window: &mut gpui::Window,
    cx: &mut gpui::Context<Zetta>,
) -> SmallVec<[gpui::AnyElement; 8]> {
    let Some(editor) = zetta.settings_editor.as_mut() else {
        return SmallVec::new();
    };
    let colors = cx.theme().colors().clone();
    let handle = cx.entity().downgrade();
    let mut elements = SmallVec::new();
    match candidate.row {
        KeymapRow::SectionHeader(section_index) => {
            if let Some(section) = editor.keymap.sections.get(section_index) {
                let context = section.context.clone();
                let focused = editor.focused_control
                    == Some(SettingsControl::Input(SettingsInput::Keymap(
                        KeymapTextField::Context(section_index),
                    )));
                let element = h_flex()
                    .w_full()
                    .h(px(KEYMAP_ROW_HEIGHT))
                    .gap_2()
                    .px_2()
                    .border_t_1()
                    .border_b_1()
                    .border_color(colors.border)
                    .bg(if focused {
                        colors.element_selected
                    } else {
                        colors.editor_background
                    })
                    .child(div().flex_none().text_sm().child("Context"))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(crate::Zetta::text_input_widget(
                                format!("settings-keymap-section-{section_index}-context"),
                                context,
                                SettingsInput::Keymap(KeymapTextField::Context(section_index)),
                                editor.focused_input,
                                colors.clone(),
                                handle.clone(),
                            )),
                    )
                    .into_any_element();
                elements.push(element);
            }
        }
        KeymapRow::AddSection => {
            let focused = editor.focused_control == Some(SettingsControl::AddKeymapSection);
            let handle_for_click = handle.clone();
            let element = h_flex()
                .w_full()
                .h(px(KEYMAP_ROW_HEIGHT))
                .pl_6()
                .pr_2()
                .border_b_1()
                .border_color(colors.border_variant)
                .child(
                    Button::new("add-keymap-section", "Add keymap context")
                        .style(ButtonStyle::Outlined)
                        .toggle_state(focused)
                        .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
                        .on_click(move |_, _, cx| {
                            handle_for_click
                                .update(cx, |zetta, cx| {
                                    if let Some(editor) = zetta.settings_editor.as_mut() {
                                        editor
                                            .keymap
                                            .sections
                                            .push(KeymapSectionForm::new("Zetta > Terminal"));
                                        editor.keymap_dirty = true;
                                        refresh_keymap_cache(editor);
                                        invalidate_controls_cache(editor);
                                        cx.notify();
                                    }
                                })
                                .ok();
                        }),
                )
                .into_any_element();
            elements.push(element);
        }
        _ => {}
    }
    elements
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
