use std::{fs, io, path::Path};

use anyhow::{Context as _, Result};
use serde_json::{Map, Value, json};

use crate::config::{Config, PaneControlsPosition};

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
    HttpServerPort,
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
    pub detected: bool,
}

#[derive(Clone, Debug)]
pub struct ConfigurationForm {
    root: Map<String, Value>,
    pub default_profile: String,
    pub working_directory: TextField,
    pub theme: String,
    pub terminal_font_size: TextField,
    pub terminal_font_family: String,
    pub max_scroll_history_lines: TextField,
    pub inactive_pane_opacity: f32,
    pub pane_controls_position: PaneControlsPosition,
    pub http_server_port: TextField,
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
                    detected,
                }
            })
            .collect();
        Ok(Self {
            default_profile: config.profiles[config.default_profile].name.clone(),
            working_directory: TextField::new(
                string("working_directory").unwrap_or_else(|| "~".to_owned()),
            ),
            theme: config
                .theme
                .clone()
                .unwrap_or_else(|| "One Light".to_owned()),
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
            pane_controls_position: config.pane_controls_position,
            http_server_port: TextField::new(config.http_server_port.to_string()),
            root,
            profiles,
        })
    }

    pub fn text_mut(&mut self, field: ConfigTextField) -> Option<&mut TextField> {
        match field {
            ConfigTextField::WorkingDirectory => Some(&mut self.working_directory),
            ConfigTextField::FontSize => Some(&mut self.terminal_font_size),
            ConfigTextField::ScrollHistory => Some(&mut self.max_scroll_history_lines),
            ConfigTextField::HttpServerPort => Some(&mut self.http_server_port),
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
            "working_directory".into(),
            json!(self.working_directory.text),
        );
        root.insert("theme".into(), json!(self.theme));
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
        root.insert(
            "pane_controls_position".into(),
            json!(self.pane_controls_position.as_str()),
        );
        let http_server_port = self
            .http_server_port
            .text
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .context("HTTP server port must be an integer from 1 to 65535")?;
        root.insert("http_server_port".into(), json!(http_server_port));
        if !self.profiles.is_empty() || root.contains_key("profiles") {
            root.insert(
                "profiles".into(),
                Value::Array(
                    self.profiles
                        .iter()
                        .filter(|profile| !profile.detected || profile.theme.is_some())
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
                            Value::Object(value)
                        })
                        .collect(),
                ),
            );
        }
        serde_json::to_string_pretty(&Value::Object(root)).context("serializing configuration")
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
        let template = serde_json::from_str(include_str!("../keymap.example.json"))
            .context("parsing bundled keymap template")?;
        let value = read_json_or(path, template)?;
        let sections = value
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
                        keystroke: TextField::new(keystroke),
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
                            .map(|binding| (binding.keystroke.text.clone(), binding.action.clone()))
                            .collect(),
                    ),
                );
                Value::Object(value)
            })
            .collect();
        serde_json::to_string_pretty(&Value::Array(sections)).context("serializing keymap")
    }
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
