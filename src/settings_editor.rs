use std::{collections::HashMap, fs, io, path::Path};

use anyhow::{Context as _, Result};
use serde_json::{Map, Value, json};
use ui::IconName;

use crate::config::{
    Config, NewTabProfile, PaneControlsPosition, WorkingDirectoryScope, profile_is_hidden,
};
use crate::startup::{keymap_keystroke_display, keymap_keystroke_storage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsPage {
    Configuration,
    Themes,
    Keymap,
}

#[derive(Clone, Debug, Default)]
pub struct TextField {
    pub text: String,
    pub cursor: usize,
    pub select_all: bool,
}

impl TextField {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            cursor: text.len(),
            text,
            select_all: false,
        }
    }

    pub fn insert(&mut self, text: &str) {
        self.delete_selection();
        let text = text.replace(['\r', '\n'], "");
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor > 0 {
            let previous = super::previous_char_boundary(&self.text, self.cursor);
            self.text.replace_range(previous..self.cursor, "");
            self.cursor = previous;
        }
    }

    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor < self.text.len() {
            let next = super::next_char_boundary(&self.text, self.cursor);
            self.text.replace_range(self.cursor..next, "");
        }
    }

    pub fn move_left(&mut self) {
        self.cursor = if self.select_all {
            0
        } else {
            super::previous_char_boundary(&self.text, self.cursor)
        };
        self.select_all = false;
    }

    pub fn move_right(&mut self) {
        self.cursor = if self.select_all {
            self.text.len()
        } else {
            super::next_char_boundary(&self.text, self.cursor)
        };
        self.select_all = false;
    }

    pub fn select_all(&mut self) {
        self.select_all = !self.text.is_empty();
    }

    fn delete_selection(&mut self) -> bool {
        if !self.select_all {
            return false;
        }
        self.text.clear();
        self.cursor = 0;
        self.select_all = false;
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigTextField {
    WorkingDirectory,
    FontSize,
    ScrollHistory,
    #[cfg(feature = "http-server")]
    HttpServerPort,
    #[cfg(feature = "tftp-server")]
    TftpServerPort,
    ProfileName(usize),
    ProfileProgram(usize),
    ProfileArguments(usize),
}

#[derive(Clone, Debug)]
pub struct ProfileForm {
    pub name: TextField,
    pub program: TextField,
    pub arguments: TextField,
    pub theme: Option<String>,
    pub hidden: bool,
    pub detected: bool,
}

#[derive(Clone, Debug)]
pub struct ConfigurationForm {
    root: Map<String, Value>,
    pub default_profile: String,
    pub new_tab_profile: NewTabProfile,
    pub working_directory: TextField,
    pub working_directory_scope: WorkingDirectoryScope,
    pub theme: String,
    pub default_tab_icon: Option<IconName>,
    pub terminal_font_size: TextField,
    pub terminal_font_family: String,
    pub max_scroll_history_lines: TextField,
    pub inactive_pane_opacity: f32,
    pub compact_mode: bool,
    pub hide_pane_size: bool,
    pub hide_title_bar_labels: bool,
    pub hide_title_bar_buttons: bool,
    #[cfg(target_os = "macos")]
    pub hide_title_bar_menus: bool,
    pub pane_controls_position: PaneControlsPosition,
    pub pane_controls_hidden_by_default: bool,
    #[cfg(feature = "http-server")]
    pub http_server_port: TextField,
    #[cfg(feature = "tftp-server")]
    pub tftp_server_port: TextField,
    pub profiles: Vec<ProfileForm>,
}

impl ConfigurationForm {
    pub fn load(path: &Path, config: &Config) -> Result<Self> {
        let root = read_json_or(path, json!({}))?
            .as_object()
            .context("configuration root must be an object")?
            .clone();
        let string = |name: &str| root.get(name).and_then(Value::as_str).map(str::to_owned);
        let configured_profiles = root
            .get("profiles")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let profiles = config
            .profiles
            .iter()
            .map(|resolved| {
                let configured = configured_profiles.iter().find_map(|profile| {
                    let profile = profile.as_object()?;
                    profile
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.eq_ignore_ascii_case(&resolved.name))
                        .then_some(profile)
                });
                let detected = configured.is_none_or(|profile| !profile.contains_key("program"));
                ProfileForm {
                    name: TextField::new(resolved.name.clone()),
                    program: TextField::new(
                        configured
                            .and_then(|profile| profile.get("program"))
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    ),
                    arguments: TextField::new(
                        configured
                            .and_then(|profile| profile.get("args"))
                            .and_then(Value::as_array)
                            .map(|args| {
                                args.iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default(),
                    ),
                    theme: configured
                        .and_then(|profile| profile.get("theme"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| resolved.theme.clone()),
                    hidden: configured
                        .and_then(|profile| profile.get("hidden"))
                        .and_then(Value::as_bool)
                        .unwrap_or_else(|| profile_is_hidden(resolved, &config.hidden_profiles)),
                    detected,
                }
            })
            .collect();
        Ok(Self {
            default_profile: config.profiles[config.default_profile].name.clone(),
            new_tab_profile: config.new_tab_profile,
            working_directory: TextField::new(
                string("working_directory").unwrap_or_else(|| "~".to_owned()),
            ),
            working_directory_scope: config.working_directory_scope,
            theme: config
                .theme
                .clone()
                .unwrap_or_else(|| crate::ZETTA_DEFAULT_THEME.to_owned()),
            default_tab_icon: config.default_tab_icon,
            terminal_font_size: TextField::new(
                config.terminal_font_size.unwrap_or(14.).to_string(),
            ),
            terminal_font_family: config.terminal_font_family.clone(),
            max_scroll_history_lines: TextField::new(
                if config.max_scroll_history_lines == terminal::MAX_SCROLL_HISTORY_LINES {
                    "Max".to_owned()
                } else {
                    config.max_scroll_history_lines.to_string()
                },
            ),
            inactive_pane_opacity: config.inactive_pane_opacity,
            compact_mode: config.compact_mode,
            hide_pane_size: config.hide_pane_size,
            hide_title_bar_labels: config.hide_title_bar_labels,
            hide_title_bar_buttons: config.hide_title_bar_buttons,
            #[cfg(target_os = "macos")]
            hide_title_bar_menus: config.hide_title_bar_menus,
            pane_controls_position: config.pane_controls_position,
            pane_controls_hidden_by_default: config.pane_controls_hidden_by_default,
            #[cfg(feature = "http-server")]
            http_server_port: TextField::new(config.http_server_port.to_string()),
            #[cfg(feature = "tftp-server")]
            tftp_server_port: TextField::new(config.tftp_server_port.to_string()),
            root,
            profiles,
        })
    }

    pub fn text_mut(&mut self, field: ConfigTextField) -> Option<&mut TextField> {
        match field {
            ConfigTextField::WorkingDirectory => Some(&mut self.working_directory),
            ConfigTextField::FontSize => Some(&mut self.terminal_font_size),
            ConfigTextField::ScrollHistory => Some(&mut self.max_scroll_history_lines),
            #[cfg(feature = "http-server")]
            ConfigTextField::HttpServerPort => Some(&mut self.http_server_port),
            #[cfg(feature = "tftp-server")]
            ConfigTextField::TftpServerPort => Some(&mut self.tftp_server_port),
            ConfigTextField::ProfileName(index) => {
                self.profiles.get_mut(index).map(|p| &mut p.name)
            }
            ConfigTextField::ProfileProgram(index) => {
                self.profiles.get_mut(index).map(|p| &mut p.program)
            }
            ConfigTextField::ProfileArguments(index) => {
                self.profiles.get_mut(index).map(|p| &mut p.arguments)
            }
        }
    }

    pub fn to_json(&self) -> Result<String> {
        let mut root = self.root.clone();
        root.insert("default_profile".into(), json!(self.default_profile));
        root.insert(
            "new_tab_profile".into(),
            json!(self.new_tab_profile.as_str()),
        );
        root.insert(
            "working_directory".into(),
            json!(self.working_directory.text),
        );
        root.insert(
            "working_directory_scope".into(),
            json!(self.working_directory_scope.as_str()),
        );
        root.insert("theme".into(), json!(self.theme));
        let default_tab_icon = self.default_tab_icon.map(|icon| {
            let name: &'static str = icon.into();
            Value::String(name.to_owned())
        });
        root.insert(
            "default_tab_icon".into(),
            default_tab_icon.unwrap_or(Value::Null),
        );
        let terminal_font_size = self
            .terminal_font_size
            .text
            .trim()
            .parse::<f32>()
            .context("terminal font size must be a number")?;
        root.insert("terminal_font_size".into(), json!(terminal_font_size));
        root.insert(
            "terminal_font_family".into(),
            json!(self.terminal_font_family),
        );
        let scroll_history = if self
            .max_scroll_history_lines
            .text
            .trim()
            .eq_ignore_ascii_case("max")
        {
            terminal::MAX_SCROLL_HISTORY_LINES as u64
        } else {
            self.max_scroll_history_lines
                .text
                .trim()
                .parse::<u64>()
                .context("scrollback history must be a non-negative integer or Max")?
        };
        root.insert("max_scroll_history_lines".into(), json!(scroll_history));
        let inactive_pane_opacity = format!("{:.2}", self.inactive_pane_opacity)
            .parse::<f64>()
            .context("formatting inactive pane opacity")?;
        root.insert("inactive_pane_opacity".into(), json!(inactive_pane_opacity));
        root.insert("compact_mode".into(), json!(self.compact_mode));
        root.insert("hide_pane_size".into(), json!(self.hide_pane_size));
        root.insert(
            "hide_title_bar_labels".into(),
            json!(self.hide_title_bar_labels),
        );
        root.insert(
            "hide_title_bar_buttons".into(),
            json!(self.hide_title_bar_buttons),
        );
        #[cfg(target_os = "macos")]
        root.insert(
            "hide_title_bar_menus".into(),
            json!(self.hide_title_bar_menus),
        );
        root.insert(
            "pane_controls_position".into(),
            json!(self.pane_controls_position.as_str()),
        );
        root.insert(
            "pane_controls_hidden_by_default".into(),
            json!(self.pane_controls_hidden_by_default),
        );
        #[cfg(feature = "http-server")]
        {
            let http_server_port = self
                .http_server_port
                .text
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .context("HTTP server port must be an integer from 1 to 65535")?;
            root.insert("http_server_port".into(), json!(http_server_port));
        }
        #[cfg(feature = "tftp-server")]
        {
            let tftp_server_port = self
                .tftp_server_port
                .text
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .context("TFTP server port must be an integer from 1 to 65535")?;
            root.insert("tftp_server_port".into(), json!(tftp_server_port));
        }
        if !self.profiles.is_empty() || root.contains_key("profiles") {
            root.insert(
                "profiles".into(),
                Value::Array(
                    self.profiles
                        .iter()
                        .filter(|profile| {
                            !profile.detected || profile.theme.is_some() || profile.hidden
                        })
                        .map(|profile| {
                            let mut value = Map::new();
                            value.insert("name".into(), json!(profile.name.text));
                            if !profile.program.text.trim().is_empty() {
                                value.insert("program".into(), json!(profile.program.text));
                                value.insert(
                                    "args".into(),
                                    Value::Array(
                                        profile
                                            .arguments
                                            .text
                                            .split(',')
                                            .map(str::trim)
                                            .filter(|arg| !arg.is_empty())
                                            .map(|arg| json!(arg))
                                            .collect(),
                                    ),
                                );
                            }
                            if let Some(theme) = &profile.theme {
                                value.insert("theme".into(), json!(theme));
                            }
                            if profile.hidden {
                                value.insert("hidden".into(), json!(true));
                            }
                            Value::Object(value)
                        })
                        .collect(),
                ),
            );
        }
        strip_default_configuration_values(&mut root, &self.profiles, &self.working_directory);
        serde_json::to_string_pretty(&Value::Object(root)).context("serializing configuration")
    }
}

fn strip_matching_defaults(root: &mut Map<String, Value>, defaults: &[(&str, Value)]) {
    for (key, default) in defaults {
        if root.get(*key) == Some(default) {
            root.remove(*key);
        }
    }
}

fn strip_default_configuration_values(
    root: &mut Map<String, Value>,
    profiles: &[ProfileForm],
    working_directory: &TextField,
) {
    #[allow(
        unused_mut,
        reason = "the platform- and feature-gated pushes below are the only mutations"
    )]
    let mut defaults: Vec<(&str, Value)> = vec![
        ("new_tab_profile", json!(NewTabProfile::default().as_str())),
        (
            "working_directory_scope",
            json!(WorkingDirectoryScope::default().as_str()),
        ),
        ("theme", json!(crate::ZETTA_DEFAULT_THEME)),
        ("default_tab_icon", json!("terminal")),
        (
            "terminal_font_family",
            json!(crate::config::DEFAULT_TERMINAL_FONT_FAMILY),
        ),
        (
            "max_scroll_history_lines",
            json!(terminal::MAX_SCROLL_HISTORY_LINES as u64),
        ),
        (
            "inactive_pane_opacity",
            json!(
                format!("{:.2}", crate::config::DEFAULT_INACTIVE_PANE_OPACITY)
                    .parse::<f64>()
                    .unwrap()
            ),
        ),
        ("compact_mode", json!(false)),
        ("hide_pane_size", json!(true)),
        ("hide_title_bar_labels", json!(false)),
        ("hide_title_bar_buttons", json!(false)),
        (
            "pane_controls_position",
            json!(PaneControlsPosition::default().as_str()),
        ),
        ("pane_controls_hidden_by_default", json!(false)),
    ];
    #[cfg(target_os = "macos")]
    defaults.push(("hide_title_bar_menus", json!(true)));
    #[cfg(feature = "http-server")]
    defaults.push(("http_server_port", json!(crate::config::DEFAULT_HTTP_PORT)));
    #[cfg(feature = "tftp-server")]
    defaults.push((
        "tftp_server_port",
        json!(crate::config::DEFAULT_TFTP_SERVER_PORT),
    ));
    strip_matching_defaults(root, &defaults);

    if root
        .get("default_profile")
        .and_then(Value::as_str)
        .zip(profiles.first())
        .is_some_and(|(name, first)| name.eq_ignore_ascii_case(&first.name.text))
    {
        root.remove("default_profile");
    }
    if matches!(working_directory.text.trim(), "~" | "~/") {
        root.remove("working_directory");
    }
}

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
}

impl KeymapSectionForm {
    pub fn new(context: impl Into<String>) -> Self {
        Self {
            extra: Map::new(),
            context: TextField::new(context),
            bindings: Vec::new(),
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
        let merged = merge_keymap_with_defaults(user_value, &default_template)?;
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
                Ok(KeymapSectionForm {
                    extra,
                    context,
                    bindings,
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
                Value::Object(value)
            })
            .collect();
        let sections = strip_default_keymap_bindings(sections);
        serde_json::to_string_pretty(&Value::Array(sections)).context("serializing keymap")
    }
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

            // Preserve other user section properties (e.g., use_key_equivalents)
            if let Some(user_obj) = user_section.as_object() {
                let mut merged_obj = merged_section.as_object().cloned().unwrap_or_default();
                for (key, value) in user_obj {
                    if key != "context" && key != "bindings" {
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

fn bundled_keymap_template() -> Result<Vec<Value>> {
    serde_json::from_str(include_str!("../keymap.example.json"))
        .context("parsing bundled keymap template")
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

pub fn save(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, format!("{text}\n")).with_context(|| format!("writing {}", path.display()))
}

fn read_json_or(path: &Path, fallback: Value) -> Result<Value> {
    match fs::read_to_string(path) {
        Ok(text) => {
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(fallback),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
#[path = "tests/settings_editor.rs"]
mod tests;
