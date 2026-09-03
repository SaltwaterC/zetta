use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::Path,
    sync::{Arc, OnceLock},
};

use anyhow::{Context as _, Result, anyhow};
use gpui::Action;
use indexmap::IndexMap;
use serde_json::{Map, Value, json};
use ui::IconName;

use crate::config::{
    Config, NewTabProfile, PaneControlsPosition, PaneSplitAxis, PaneSplitCommand,
    PaneSplitOverlaySize, PaneSplitTemplate, PaneSplitTemplateConfig, SessionRetention,
    WorkingDirectoryScope, built_in_pane_split_templates, is_valid_pane_split_label,
    profile_is_hidden,
};
use crate::pane::MAX_PANES_PER_TAB;
use crate::profile_icon::ProfileIcon;
use crate::startup::{keymap_keystroke_display, keymap_keystroke_storage};
use crate::text_edit::TextField;

mod configuration;
mod keymap;
mod pane_templates;

// The three forms are named `crate::settings_editor::…` by the settings UI and
// its view, exactly as they were before the split.
pub(crate) use configuration::*;
pub(crate) use keymap::*;
pub(crate) use pane_templates::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsPage {
    Configuration,
    Themes,
    Keymap,
    PaneTemplates,
    Projects,
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
