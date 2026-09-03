//! The typed form behind the Keymap page, and its serialization.
//!
//! The user's keymap is merged with the default template so the page can show
//! every binding, and bindings identical to the default are stripped on write
//! so the file records only what was rebound.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeymapTextField {
    Context(usize),
    Keystroke(usize, usize),
}

#[derive(Clone, Debug)]
pub struct BindingForm {
    pub keystroke: TextField,
    pub action: Value,
}

impl BindingForm {
    pub fn action_name(&self) -> String {
        match &self.action {
            Value::String(action) => action.clone(),
            Value::Array(action) => action
                .first()
                .and_then(Value::as_str)
                .unwrap_or("Parameterized action")
                .to_owned(),
            Value::Null => "Unbound".to_owned(),
            action => action.to_string(),
        }
    }

    pub fn action_parameter(&self, name: &str) -> Option<String> {
        self.action
            .as_array()?
            .get(1)?
            .as_object()?
            .get(name)?
            .as_str()
            .map(str::to_owned)
    }

    pub fn action_usize_parameter(&self, name: &str) -> Option<usize> {
        self.action
            .as_array()?
            .get(1)?
            .as_object()?
            .get(name)?
            .as_u64()?
            .try_into()
            .ok()
    }
}

#[derive(Clone, Debug)]
pub struct KeymapSectionForm {
    extra: Map<String, Value>,
    pub context: TextField,
    pub bindings: Vec<BindingForm>,
    pub unbind: IndexMap<String, String>,
    pub unbound_defaults: Vec<BindingForm>,
}

impl KeymapSectionForm {
    pub fn new(context: impl Into<String>) -> Self {
        Self {
            extra: Map::new(),
            context: TextField::new(context),
            bindings: Vec::new(),
            unbind: IndexMap::new(),
            unbound_defaults: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct KeymapForm {
    pub sections: Vec<KeymapSectionForm>,
}

impl KeymapForm {
    pub fn load(path: &Path) -> Result<Self> {
        let default_template = bundled_keymap_template()?;
        let user_value = read_json_or(path, Value::Array(vec![]))?;
        let merged = merge_keymap_with_defaults(user_value, default_template)?;
        let sections = merged
            .as_array()
            .context("keymap root must be an array")?
            .iter()
            .map(|section| {
                let mut extra = section
                    .as_object()
                    .context("each keymap section must be an object")?
                    .clone();
                let context = TextField::new(
                    extra
                        .remove("context")
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_default(),
                );
                let bindings = extra
                    .remove("bindings")
                    .and_then(|value| value.as_object().cloned())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(keystroke, action)| BindingForm {
                        keystroke: TextField::new(keymap_keystroke_display(&keystroke)),
                        action,
                    })
                    .collect();
                let unbind: IndexMap<String, String> = extra
                    .remove("unbind")
                    .and_then(|value| value.as_object().cloned())
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|(keystroke, action)| {
                        action
                            .as_str()
                            .map(|a| (keymap_keystroke_storage(&keystroke), a.to_owned()))
                    })
                    .collect();
                // Build unbound_defaults from default template for this context
                let context_str = context.text.clone();
                let defaults_by_context = default_bindings_by_context().ok();
                let unbound_defaults = if let Some(defaults_by_context) =
                    defaults_by_context.as_ref()
                    && let Some(default_bindings) = defaults_by_context.get(&context_str)
                {
                    default_bindings
                        .iter()
                        .filter_map(|(storage_keystroke, action)| {
                            if unbind.contains_key(storage_keystroke) {
                                // Find the display form of the keystroke
                                let display_keystroke = keymap_keystroke_display(storage_keystroke);
                                Some(BindingForm {
                                    keystroke: TextField::new(display_keystroke),
                                    action: action.clone(),
                                })
                            } else {
                                None
                            }
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                Ok(KeymapSectionForm {
                    extra,
                    context,
                    bindings,
                    unbind,
                    unbound_defaults,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { sections })
    }

    pub fn text_mut(&mut self, field: KeymapTextField) -> Option<&mut TextField> {
        match field {
            KeymapTextField::Context(section) => {
                self.sections.get_mut(section).map(|s| &mut s.context)
            }
            KeymapTextField::Keystroke(section, binding) => self
                .sections
                .get_mut(section)?
                .bindings
                .get_mut(binding)
                .map(|binding| &mut binding.keystroke),
        }
    }

    pub fn to_json(&self) -> Result<String> {
        let sections = self
            .sections
            .iter()
            .map(|section| {
                let mut value = section.extra.clone();
                value.insert("context".into(), json!(section.context.text));
                value.insert(
                    "bindings".into(),
                    Value::Object(
                        section
                            .bindings
                            .iter()
                            .map(|binding| {
                                (
                                    keymap_keystroke_display(&binding.keystroke.text),
                                    binding.action.clone(),
                                )
                            })
                            .collect(),
                    ),
                );
                if !section.unbind.is_empty() {
                    value.insert(
                        "unbind".into(),
                        Value::Object(
                            section
                                .unbind
                                .iter()
                                .map(|(keystroke, action)| {
                                    (keymap_keystroke_storage(keystroke), json!(action))
                                })
                                .collect(),
                        ),
                    );
                }
                Value::Object(value)
            })
            .collect();
        let sections = strip_default_keymap_bindings(sections);
        serde_json::to_string_pretty(&Value::Array(sections)).context("serializing keymap")
    }
}

/// Renames every parameterized pane-template binding that points at one of
/// the supplied old names. Returns whether the keymap changed.
pub(crate) fn rename_pane_template_bindings(
    keymap: &mut KeymapForm,
    renames: &[(String, String)],
) -> bool {
    let action_name = crate::ApplyPaneSplitTemplate::name_for_type();
    let mut changed = false;
    for section in &mut keymap.sections {
        for binding in &mut section.bindings {
            let Some(action) = binding.action.as_array_mut() else {
                continue;
            };
            if action.first().and_then(Value::as_str) != Some(action_name) {
                continue;
            }
            let Some(arguments) = action.get_mut(1).and_then(Value::as_object_mut) else {
                continue;
            };
            let Some(current) = arguments.get("name").and_then(Value::as_str) else {
                continue;
            };
            if let Some((_, new_name)) = renames.iter().find(|(old_name, _)| old_name == current) {
                arguments.insert("name".to_owned(), Value::String(new_name.clone()));
                changed = true;
            }
        }
    }
    changed
}

/// Merges user keymap with default template, with user bindings overriding defaults.
fn merge_keymap_with_defaults(user_value: Value, default_template: &[Value]) -> Result<Value> {
    let mut merged: Vec<Value> = default_template.to_vec();

    let user_sections = user_value
        .as_array()
        .context("keymap root must be an array")?;

    // Build lookup of default sections by context
    let mut defaults_by_context: HashMap<&str, &Value> = HashMap::new();
    for section in default_template {
        if let Some(context) = section.get("context").and_then(|v| v.as_str()) {
            defaults_by_context.insert(context, section);
        }
    }

    // Apply user customizations to existing default sections
    for user_section in user_sections {
        let user_context = user_section
            .get("context")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if let Some(default_section) = defaults_by_context.get(user_context) {
            // Start with a cloned owned Value from the default
            let mut merged_section = (*default_section).clone();

            // Merge bindings: user bindings override defaults
            if let Some(user_bindings) = user_section.get("bindings").and_then(|v| v.as_object()) {
                let mut bindings = merged_section
                    .get("bindings")
                    .and_then(|v| v.as_object().cloned())
                    .unwrap_or_default();

                // First, remove any default bindings that have the same action as user bindings
                // This ensures rebinding an action replaces the old keybinding
                let user_actions: std::collections::HashSet<&Value> =
                    user_bindings.values().collect();
                bindings.retain(|_, action| !user_actions.contains(action));

                // Then add user bindings
                bindings.extend(user_bindings.clone());
                merged_section["bindings"] = Value::Object(bindings);
            }

            // Merge unbind: user unbind entries are added to defaults
            if let Some(user_unbind) = user_section.get("unbind").and_then(|v| v.as_object()) {
                let mut unbind = merged_section
                    .get("unbind")
                    .and_then(|v| v.as_object().cloned())
                    .unwrap_or_default();
                unbind.extend(user_unbind.clone());
                merged_section["unbind"] = Value::Object(unbind);

                // Remove default bindings that are explicitly unbound
                if let Some(Value::Object(bindings)) = merged_section.get_mut("bindings") {
                    for (unbind_keystroke, _) in user_unbind {
                        let normalized = keymap_keystroke_storage(unbind_keystroke);
                        bindings.retain(|keystroke, _| {
                            keymap_keystroke_storage(keystroke) != normalized
                        });
                    }
                }
            }

            // Preserve other user section properties (e.g., use_key_equivalents)
            if let Some(user_obj) = user_section.as_object() {
                let mut merged_obj = merged_section.as_object().cloned().unwrap_or_default();
                for (key, value) in user_obj {
                    if key != "context" && key != "bindings" && key != "unbind" {
                        merged_obj.insert(key.clone(), value.clone());
                    }
                }
                merged_section = Value::Object(merged_obj);
            }

            // Replace in merged list
            if let Some(idx) = merged
                .iter()
                .position(|s| s.get("context").and_then(|v| v.as_str()) == Some(user_context))
            {
                merged[idx] = merged_section;
            }
        } else {
            // New section not in defaults - add it
            merged.push(user_section.clone());
        }
    }

    Ok(Value::Array(merged))
}

/// A lookup of default bindings as context -> (storage keystroke -> action).
pub type DefaultBindingsByContext = HashMap<String, IndexMap<String, Value>>;

/// The parsed bundled template, parsed at most once per process. The payload is
/// compiled in, so the parse result never changes.
pub fn bundled_keymap_template() -> Result<&'static [Value]> {
    static TEMPLATE: OnceLock<std::result::Result<Vec<Value>, String>> = OnceLock::new();
    match TEMPLATE.get_or_init(|| {
        serde_json::from_str::<Vec<Value>>(include_str!("../../keymap.example.json"))
            .map_err(|error| format!("parsing bundled keymap template: {error}"))
    }) {
        Ok(template) => Ok(template.as_slice()),
        Err(error) => Err(anyhow!("{error}")),
    }
}

/// A lookup map of default bindings by context for efficient checking, built at
/// most once per process. `is_default_binding` consults this once per keymap row
/// on every settings frame, so rebuilding it per call would reparse the bundled
/// template hundreds of times per frame.
pub fn default_bindings_by_context() -> Result<&'static DefaultBindingsByContext> {
    static BINDINGS: OnceLock<DefaultBindingsByContext> = OnceLock::new();
    if let Some(bindings) = BINDINGS.get() {
        return Ok(bindings);
    }
    let template = bundled_keymap_template()?;
    let mut map = DefaultBindingsByContext::new();
    for section in template {
        if let Some(context) = section.get("context").and_then(|v| v.as_str())
            && let Some(bindings) = section.get("bindings").and_then(|v| v.as_object())
        {
            let mut section_bindings = IndexMap::new();
            for (keystroke, action) in bindings {
                section_bindings.insert(keymap_keystroke_storage(keystroke), action.clone());
            }
            map.insert(context.to_owned(), section_bindings);
        }
    }
    Ok(BINDINGS.get_or_init(|| map))
}

/// A keymap section's `bindings` map paired with everything else in the
/// section (e.g. `use_key_equivalents`), with `context` and `bindings`
/// removed from the latter.
type KeymapSectionParts = (Map<String, Value>, Map<String, Value>);

fn split_keymap_section(section: &Value) -> Option<KeymapSectionParts> {
    let mut extra = section.as_object()?.clone();
    extra.remove("context");
    let bindings = extra
        .remove("bindings")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    Some((bindings, extra))
}

fn strip_default_keymap_bindings(sections: Vec<Value>) -> Vec<Value> {
    let defaults = bundled_keymap_template().unwrap_or_default();
    let defaults_by_context: HashMap<&str, KeymapSectionParts> = defaults
        .iter()
        .filter_map(|section| {
            let context = section.get("context")?.as_str()?;
            Some((context, split_keymap_section(section)?))
        })
        .collect();

    sections
        .into_iter()
        .filter_map(|section| {
            let mut object = section.as_object()?.clone();
            let context = object.get("context").and_then(Value::as_str)?.to_owned();
            let default = defaults_by_context.get(context.as_str());

            if let Some((default_bindings, _)) = default
                && let Some(Value::Object(bindings)) = object.get_mut("bindings")
            {
                bindings.retain(|keystroke, action| {
                    let normalized = keymap_keystroke_storage(keystroke);
                    !default_bindings
                        .iter()
                        .any(|(default_keystroke, default_action)| {
                            keymap_keystroke_storage(default_keystroke) == normalized
                                && default_action == action
                        })
                });
            }

            let bindings_empty = object
                .get("bindings")
                .and_then(Value::as_object)
                .is_some_and(Map::is_empty);
            let extra_matches_default = default.is_some_and(|(_, default_extra)| {
                let mut extra = object.clone();
                extra.remove("context");
                extra.remove("bindings");
                &extra == default_extra
            });

            if bindings_empty && extra_matches_default {
                None
            } else {
                Some(Value::Object(object))
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "../tests/settings_editor/keymap.rs"]
mod tests;
