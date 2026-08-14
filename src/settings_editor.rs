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
    Config, NewTabProfile, PaneControlsPosition, PaneSplitAxis, PaneSplitOverlaySize,
    PaneSplitTemplate, PaneSplitTemplateConfig, WorkingDirectoryScope,
    built_in_pane_split_templates, is_valid_pane_split_label, profile_is_hidden,
};
use crate::profile_icon::ProfileIcon;
use crate::startup::{keymap_keystroke_display, keymap_keystroke_storage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsPage {
    Configuration,
    Themes,
    Keymap,
    PaneTemplates,
    Projects,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
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

/// A compact, copyable path into a recursive pane-template tree.
///
/// Each bit records whether the corresponding child is the second child. A
/// template with at most 64 leaves can never need more than 63 split edges,
/// so a `u64` is sufficient while keeping settings controls cheap to clone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PaneTemplateNodePath {
    bits: u64,
    length: u8,
}

impl PaneTemplateNodePath {
    pub const ROOT: Self = Self { bits: 0, length: 0 };

    #[allow(dead_code)]
    pub fn root() -> Self {
        Self::ROOT
    }

    pub fn depth(self) -> usize {
        self.length as usize
    }

    pub fn is_root(self) -> bool {
        self.length == 0
    }

    pub fn child(self, second: bool) -> Option<Self> {
        // Lazily, for the same reason as `parent`: the discarded value must not
        // be computed when the path is already at its depth limit.
        (self.length < 64).then(|| Self {
            bits: (self.bits << 1) | u64::from(second),
            length: self.length + 1,
        })
    }

    /// The path of this node's containing split, or `None` at the root.
    ///
    /// `checked_sub` rather than a `length > 0` guard because `then_some` takes
    /// its value eagerly: the root's `length - 1` was evaluated even when the
    /// result was discarded, which panicked in debug builds and wrapped to a
    /// 255-deep path in release ones.
    pub fn parent(self) -> Option<Self> {
        self.length.checked_sub(1).map(|length| Self {
            bits: self.bits >> 1,
            length,
        })
    }

    pub fn segment(self, index: usize) -> Option<bool> {
        if index >= self.depth() {
            return None;
        }
        Some(((self.bits >> (self.length as usize - index - 1)) & 1) != 0)
    }

    fn suffix(self, start: usize) -> Option<Self> {
        if start > self.depth() {
            return None;
        }
        let length = self.depth() - start;
        let bits = match length {
            0 => 0,
            64 => self.bits,
            _ => self.bits & ((1_u64 << length) - 1),
        };
        Some(Self {
            bits,
            length: length as u8,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneTemplateNodeField {
    Label,
    CommandProgram,
    CommandArgument(usize),
    EnvironmentName(usize),
    EnvironmentValue(usize),
    OverlayText,
    OverlayOpacity,
    OverlayColor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneTemplateTextField {
    Name(usize),
    GlobalEnvironmentName(usize, usize),
    GlobalEnvironmentValue(usize, usize),
    Node(usize, PaneTemplateNodePath, PaneTemplateNodeField),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneTemplateCommandForm {
    pub program: TextField,
    pub args: Vec<TextField>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneTemplateSourceForm {
    Inherit,
    Profile(String),
    Command(PaneTemplateCommandForm),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneTemplateEnvironmentForm {
    pub name: TextField,
    pub value: TextField,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneTemplateOverlayForm {
    pub text: TextField,
    pub size: Option<PaneSplitOverlaySize>,
    pub opacity: TextField,
    pub color: TextField,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneTemplatePaneForm {
    pub label: TextField,
    pub source: PaneTemplateSourceForm,
    pub theme: Option<String>,
    pub environment: Vec<PaneTemplateEnvironmentForm>,
    pub overlay: Option<PaneTemplateOverlayForm>,
}

// Pane leaves are the common case and are kept inline to avoid one heap
// allocation per leaf in the bounded editor tree. Split children are already
// boxed, and the tree is capped at 64 panes.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneTemplateNodeForm {
    Pane(PaneTemplatePaneForm),
    Split {
        axis: PaneSplitAxis,
        first: Box<PaneTemplateNodeForm>,
        second: Box<PaneTemplateNodeForm>,
    },
}

/// The layout and environment a template has in the layer the form overlays:
/// the built-in presets for the user configuration, or the resolved user
/// configuration for a project. Held beside the editable copy so discarding an
/// override restores it without re-reading that layer, and shared behind an
/// `Arc` because the whole form is cloned to validate it off the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneTemplateInheritedSource {
    pub(crate) environment: Vec<PaneTemplateEnvironmentForm>,
    pub(crate) node: PaneTemplateNodeForm,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneTemplateForm {
    pub name: TextField,
    pub(crate) original_name: String,
    pub(crate) overridden: bool,
    /// `Some` when the layer below the form already provides this template.
    pub(crate) inherited_source: Option<Arc<PaneTemplateInheritedSource>>,
    pub environment: Vec<PaneTemplateEnvironmentForm>,
    pub node: PaneTemplateNodeForm,
}

impl PaneTemplateForm {
    pub fn inherited(&self) -> bool {
        self.inherited_source.is_some()
    }

    pub fn editable(&self) -> bool {
        !self.inherited() || self.overridden
    }

    pub fn is_pristine_inherited(&self) -> bool {
        self.inherited() && !self.overridden
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneTemplatesForm {
    pub templates: Vec<PaneTemplateForm>,
    pub selected_template: usize,
    pub selected_node: Option<PaneTemplateNodePath>,
    pub(crate) available_profiles: Vec<String>,
}

const BUILT_IN_PANE_TEMPLATE_NAMES: [&str; 4] =
    ["three-right", "three-left", "quarters", "four-vertical"];

fn configured_templates(value: Option<&Value>) -> Result<Option<&Map<String, Value>>> {
    value
        .map(|value| {
            value
                .as_object()
                .context("pane_split_templates must be an object")
        })
        .transpose()
}

/// The inherited layer's template names, with the built-in presets first in
/// their canonical order so the template list stays stable, then everything the
/// layer adds in name order.
fn inherited_template_names(inherited: &HashMap<String, PaneSplitTemplateConfig>) -> Vec<String> {
    let mut names = BUILT_IN_PANE_TEMPLATE_NAMES
        .iter()
        .filter(|name| inherited.contains_key(**name))
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    let mut added = inherited
        .keys()
        .filter(|name| !BUILT_IN_PANE_TEMPLATE_NAMES.contains(&name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    added.sort();
    names.extend(added);
    names
}

impl Default for PaneTemplatePaneForm {
    fn default() -> Self {
        Self {
            label: TextField::default(),
            source: PaneTemplateSourceForm::Inherit,
            theme: None,
            environment: Vec::new(),
            overlay: None,
        }
    }
}

impl PaneTemplateNodeForm {
    pub fn empty_two_pane() -> Self {
        Self::Split {
            axis: PaneSplitAxis::Vertical,
            first: Box::new(Self::Pane(PaneTemplatePaneForm::default())),
            second: Box::new(Self::Pane(PaneTemplatePaneForm::default())),
        }
    }

    pub fn pane_count(&self) -> usize {
        match self {
            Self::Pane(_) => 1,
            Self::Split { first, second, .. } => first.pane_count() + second.pane_count(),
        }
    }

    pub(crate) fn node_at(&self, path: PaneTemplateNodePath) -> Option<&Self> {
        let mut node = self;
        for index in 0..path.depth() {
            let second = path.segment(index)?;
            node = match node {
                Self::Split {
                    first,
                    second: right,
                    ..
                } => {
                    if second {
                        right
                    } else {
                        first
                    }
                }
                Self::Pane(_) => return None,
            };
        }
        Some(node)
    }

    pub(crate) fn node_at_mut(&mut self, path: PaneTemplateNodePath) -> Option<&mut Self> {
        let mut node = self;
        for index in 0..path.depth() {
            let second = path.segment(index)?;
            node = match node {
                Self::Split {
                    first,
                    second: right,
                    ..
                } => {
                    if second {
                        right
                    } else {
                        first
                    }
                }
                Self::Pane(_) => return None,
            };
        }
        Some(node)
    }

    fn split_leaf(&mut self, path: PaneTemplateNodePath, axis: PaneSplitAxis) -> bool {
        let Some(node) = self.node_at_mut(path) else {
            return false;
        };
        if !matches!(node, Self::Pane(_)) {
            return false;
        }
        *node = Self::Split {
            axis,
            first: Box::new(Self::Pane(PaneTemplatePaneForm::default())),
            second: Box::new(Self::Pane(PaneTemplatePaneForm::default())),
        };
        true
    }

    fn remove_at(&mut self, path: PaneTemplateNodePath) -> bool {
        if path.is_root() {
            return false;
        }
        Self::remove_inner(self, path)
    }

    fn remove_inner(node: &mut Self, path: PaneTemplateNodePath) -> bool {
        let Some(second) = path.segment(0) else {
            return false;
        };
        let Some(rest) = path.suffix(1) else {
            return false;
        };
        if rest.is_root() {
            let Self::Split {
                first,
                second: right,
                ..
            } = node
            else {
                return false;
            };
            *node = if second {
                *first.clone()
            } else {
                *right.clone()
            };
            return true;
        }
        let Self::Split {
            first,
            second: right,
            ..
        } = node
        else {
            return false;
        };
        Self::remove_inner(if second { right } else { first }, rest)
    }

    fn swap_children(&mut self, path: PaneTemplateNodePath) -> bool {
        let Some(Self::Split { first, second, .. }) = self.node_at_mut(path) else {
            return false;
        };
        std::mem::swap(first, second);
        true
    }

    fn set_axis(&mut self, path: PaneTemplateNodePath, axis: PaneSplitAxis) -> bool {
        let Some(Self::Split { axis: current, .. }) = self.node_at_mut(path) else {
            return false;
        };
        *current = axis;
        true
    }

    fn from_template(template: &PaneSplitTemplate) -> Self {
        match template {
            PaneSplitTemplate::Pane(pane) => Self::Pane(PaneTemplatePaneForm {
                label: TextField::new(pane.label.clone().unwrap_or_default()),
                source: if let Some(profile) = &pane.profile {
                    PaneTemplateSourceForm::Profile(profile.name.clone())
                } else if let Some(command) = &pane.command {
                    PaneTemplateSourceForm::Command(PaneTemplateCommandForm {
                        program: TextField::new(command.program.clone()),
                        args: command.args.iter().cloned().map(TextField::new).collect(),
                    })
                } else {
                    PaneTemplateSourceForm::Inherit
                },
                theme: pane.theme.clone(),
                environment: {
                    let mut environment = pane
                        .env
                        .iter()
                        .map(|(name, value)| PaneTemplateEnvironmentForm {
                            name: TextField::new(name),
                            value: TextField::new(value),
                        })
                        .collect::<Vec<_>>();
                    environment.sort_by(|left, right| left.name.text.cmp(&right.name.text));
                    environment
                },
                overlay: pane
                    .overlay
                    .as_ref()
                    .map(|overlay| PaneTemplateOverlayForm {
                        text: TextField::new(overlay.text.clone().unwrap_or_default()),
                        size: overlay.size,
                        opacity: TextField::new(
                            overlay
                                .opacity
                                .map(|opacity| opacity.to_string())
                                .unwrap_or_default(),
                        ),
                        color: TextField::new(overlay.color.clone().unwrap_or_default()),
                    }),
            }),
            PaneSplitTemplate::Split {
                axis,
                first,
                second,
            } => Self::Split {
                axis: *axis,
                first: Box::new(Self::from_template(first)),
                second: Box::new(Self::from_template(second)),
            },
        }
    }

    fn validate(&self, path: PaneTemplateNodePath, profiles: &[String]) -> Result<()> {
        match self {
            Self::Pane(pane) => {
                if !pane.label.text.is_empty() {
                    anyhow::ensure!(
                        is_valid_pane_split_label(&pane.label.text),
                        "pane {} label must be lowercase kebab-case",
                        path_label(path)
                    );
                }
                match &pane.source {
                    PaneTemplateSourceForm::Inherit => {}
                    PaneTemplateSourceForm::Profile(profile) => {
                        anyhow::ensure!(
                            profiles
                                .iter()
                                .any(|candidate| candidate.eq_ignore_ascii_case(profile)),
                            "pane {} profile {profile:?} is not available",
                            path_label(path)
                        );
                    }
                    PaneTemplateSourceForm::Command(command) => {
                        anyhow::ensure!(
                            !command.program.text.trim().is_empty(),
                            "pane {} command program is required",
                            path_label(path)
                        );
                    }
                }
                validate_pane_template_environment(
                    &pane.environment,
                    &format!("pane {}", path_label(path)),
                )?;
                if let Some(overlay) = &pane.overlay {
                    if !overlay.opacity.text.trim().is_empty() {
                        let opacity = overlay
                            .opacity
                            .text
                            .trim()
                            .parse::<u8>()
                            .context("overlay opacity must be an integer from 0 to 100")?;
                        anyhow::ensure!(
                            opacity <= 100,
                            "pane {} overlay opacity must be between 0 and 100",
                            path_label(path)
                        );
                    }
                    if !overlay.color.text.trim().is_empty() {
                        anyhow::ensure!(
                            crate::pane::overlay_color_from_value(&overlay.color.text).is_some(),
                            "pane {} overlay color must be a named color or valid hex color",
                            path_label(path)
                        );
                    }
                }
            }
            Self::Split { first, second, .. } => {
                first.validate(path.child(false).unwrap_or(path), profiles)?;
                second.validate(path.child(true).unwrap_or(path), profiles)?;
            }
        }
        Ok(())
    }

    fn to_value(&self) -> Result<Value> {
        match self {
            Self::Pane(pane) => {
                let mut object = Map::new();
                if !pane.label.text.is_empty() {
                    object.insert("label".into(), json!(pane.label.text));
                }
                match &pane.source {
                    PaneTemplateSourceForm::Inherit => {}
                    PaneTemplateSourceForm::Profile(profile) => {
                        object.insert("profile".into(), json!(profile));
                    }
                    PaneTemplateSourceForm::Command(command) => {
                        let mut command_object = Map::new();
                        command_object.insert("program".into(), json!(command.program.text));
                        let args = command
                            .args
                            .iter()
                            .map(|argument| json!(argument.text))
                            .collect::<Vec<_>>();
                        if !args.is_empty() {
                            command_object.insert("args".into(), Value::Array(args));
                        }
                        object.insert("command".into(), Value::Object(command_object));
                    }
                }
                if let Some(theme) = &pane.theme
                    && !theme.is_empty()
                {
                    object.insert("theme".into(), json!(theme));
                }
                if !pane.environment.is_empty() {
                    let mut environment = Map::new();
                    for entry in &pane.environment {
                        environment.insert(entry.name.text.clone(), json!(entry.value.text));
                    }
                    object.insert("env".into(), Value::Object(environment));
                }
                if let Some(overlay) = &pane.overlay {
                    let mut overlay_object = Map::new();
                    if !overlay.text.text.is_empty() {
                        overlay_object.insert("text".into(), json!(overlay.text.text));
                    }
                    if let Some(size) = overlay.size {
                        overlay_object.insert("size".into(), json!(size.as_str()));
                    }
                    if !overlay.opacity.text.trim().is_empty() {
                        overlay_object.insert(
                            "opacity".into(),
                            json!(overlay.opacity.text.trim().parse::<u8>()?),
                        );
                    }
                    if !overlay.color.text.trim().is_empty() {
                        overlay_object.insert("color".into(), json!(overlay.color.text));
                    }
                    object.insert("overlay".into(), Value::Object(overlay_object));
                }
                Ok(Value::Object(object))
            }
            Self::Split {
                axis,
                first,
                second,
            } => Ok(json!({axis.as_str(): [first.to_value()?, second.to_value()?]})),
        }
    }
}

fn path_label(path: PaneTemplateNodePath) -> String {
    if path.is_root() {
        return "root".to_owned();
    }
    let mut label = String::new();
    for index in 0..path.depth() {
        label.push(if path.segment(index).unwrap_or(false) {
            'R'
        } else {
            'L'
        });
    }
    label
}

fn validate_pane_template_environment(
    environment: &[PaneTemplateEnvironmentForm],
    owner: &str,
) -> Result<()> {
    let mut names = HashSet::new();
    for entry in environment {
        anyhow::ensure!(
            !entry.name.text.is_empty() && !entry.name.text.contains(['=', '\0']),
            "{owner} environment names must not be empty or contain '='"
        );
        anyhow::ensure!(
            names.insert(entry.name.text.clone()),
            "{owner} contains duplicate environment key {:?}",
            entry.name.text
        );
        anyhow::ensure!(
            !entry.value.text.contains('\0'),
            "{owner} environment values must not contain NUL"
        );
    }
    Ok(())
}

fn pane_template_environment_form(
    environment: &HashMap<String, String>,
) -> Vec<PaneTemplateEnvironmentForm> {
    let mut rows = environment
        .iter()
        .map(|(name, value)| PaneTemplateEnvironmentForm {
            name: TextField::new(name),
            value: TextField::new(value),
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| left.name.text.cmp(&right.name.text));
    rows
}

fn pane_template_environment_value(environment: &[PaneTemplateEnvironmentForm]) -> Value {
    Value::Object(
        environment
            .iter()
            .map(|entry| (entry.name.text.clone(), json!(entry.value.text)))
            .collect(),
    )
}

impl PaneTemplatesForm {
    /// The user configuration's templates, which overlay the built-in presets.
    pub fn load(value: Option<&Value>, config: &Config) -> Result<Self> {
        Ok(Self::from_layers(
            configured_templates(value)?,
            config,
            &built_in_pane_split_templates(),
        ))
    }

    /// A project overlay's templates. The resolved user configuration is the
    /// inherited layer, so a project sees the user's own templates as read-only
    /// presets alongside the built-ins and can override either.
    pub fn load_overlay(value: Option<&Value>, base: &Config, effective: &Config) -> Result<Self> {
        Ok(Self::from_layers(
            configured_templates(value)?,
            effective,
            &base.pane_split_templates,
        ))
    }

    /// Builds the form from `configured` (the overlay file's own
    /// `pane_split_templates`, which decides what is editable) and `effective`
    /// (the same file resolved against everything below it, which supplies the
    /// values), with `inherited` as the layer being overlaid.
    fn from_layers(
        configured: Option<&Map<String, Value>>,
        effective: &Config,
        inherited: &HashMap<String, PaneSplitTemplateConfig>,
    ) -> Self {
        let mut templates = Vec::new();
        for name in inherited_template_names(inherited) {
            let Some(template) = inherited.get(&name) else {
                continue;
            };
            let overridden = configured.is_some_and(|values| values.contains_key(&name));
            let source = if overridden {
                effective
                    .pane_split_templates
                    .get(&name)
                    .unwrap_or(template)
            } else {
                template
            };
            templates.push(PaneTemplateForm {
                name: TextField::new(name.clone()),
                original_name: name,
                overridden,
                inherited_source: Some(Arc::new(PaneTemplateInheritedSource {
                    environment: pane_template_environment_form(&template.env),
                    node: PaneTemplateNodeForm::from_template(&template.layout),
                })),
                environment: pane_template_environment_form(&source.env),
                node: PaneTemplateNodeForm::from_template(&source.layout),
            });
        }

        let mut custom_names = configured
            .into_iter()
            .flat_map(|values| values.keys())
            .filter(|name| !inherited.contains_key(name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        custom_names.sort();
        for name in custom_names {
            let Some(template) = effective.pane_split_templates.get(&name) else {
                continue;
            };
            templates.push(PaneTemplateForm {
                name: TextField::new(name.clone()),
                original_name: name,
                overridden: true,
                inherited_source: None,
                environment: pane_template_environment_form(&template.env),
                node: PaneTemplateNodeForm::from_template(&template.layout),
            });
        }

        Self {
            templates,
            selected_template: 0,
            selected_node: Some(PaneTemplateNodePath::ROOT),
            available_profiles: effective
                .profiles
                .iter()
                .map(|profile| profile.name.clone())
                .collect(),
        }
    }

    pub fn names(&self) -> Vec<String> {
        self.templates
            .iter()
            .map(|template| template.name.text.clone())
            .collect()
    }

    /// Resolves a stored template name against the form, following a rename
    /// made in this editing session. Returns `None` when nothing in the form
    /// answers to the name any more.
    pub fn current_name_for<'form>(&'form self, name: &str) -> Option<&'form str> {
        if let Some(template) = self
            .templates
            .iter()
            .find(|template| template.name.text == name)
        {
            return Some(template.name.text.as_str());
        }
        self.templates
            .iter()
            .find(|template| template.original_name == name)
            .map(|template| template.name.text.as_str())
    }

    pub fn selected(&self) -> Option<&PaneTemplateForm> {
        self.templates.get(self.selected_template)
    }

    pub fn selected_mut(&mut self) -> Option<&mut PaneTemplateForm> {
        self.templates.get_mut(self.selected_template)
    }

    pub fn selected_node(&self) -> Option<&PaneTemplateNodeForm> {
        self.selected()?.node.node_at(self.selected_node?)
    }

    pub fn select_template(&mut self, index: usize) -> bool {
        if index >= self.templates.len() {
            return false;
        }
        self.selected_template = index;
        self.selected_node = Some(PaneTemplateNodePath::ROOT);
        true
    }

    pub fn select_node(&mut self, path: PaneTemplateNodePath) -> bool {
        if self
            .selected()
            .is_some_and(|template| template.node.node_at(path).is_some())
        {
            self.selected_node = Some(path);
            true
        } else {
            false
        }
    }

    pub fn toggle_node_selection(&mut self, path: PaneTemplateNodePath) -> bool {
        if self
            .selected()
            .is_none_or(|template| template.node.node_at(path).is_none())
        {
            return false;
        }
        self.selected_node = if self.selected_node == Some(path) {
            path.parent()
        } else {
            Some(path)
        };
        true
    }

    pub fn selected_is_editable(&self) -> bool {
        self.selected().is_some_and(PaneTemplateForm::editable)
    }

    pub fn split_selected_leaf(&mut self, axis: PaneSplitAxis) -> Result<()> {
        anyhow::ensure!(
            self.selected_is_editable(),
            "read-only pane templates cannot be edited"
        );
        let path = self
            .selected_node
            .context("no pane-template node is selected")?;
        let template = self
            .selected_mut()
            .context("no pane template is selected")?;
        anyhow::ensure!(
            template.node.pane_count() < 64,
            "pane templates may contain at most 64 panes"
        );
        anyhow::ensure!(
            template.node.split_leaf(path, axis),
            "the selected node is not a pane leaf"
        );
        Ok(())
    }

    pub fn remove_selected_node(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.selected_is_editable(),
            "read-only pane templates cannot be edited"
        );
        anyhow::ensure!(
            !self
                .selected_node
                .is_some_and(PaneTemplateNodePath::is_root),
            "the template root cannot be removed"
        );
        let path = self
            .selected_node
            .context("no pane-template node is selected")?;
        let template = self
            .selected_mut()
            .context("no pane template is selected")?;
        let removed_panes = template
            .node
            .node_at(path)
            .map(PaneTemplateNodeForm::pane_count)
            .context("the selected pane-template node does not exist")?;
        anyhow::ensure!(
            template.node.pane_count().saturating_sub(removed_panes) >= 2,
            "pane templates must contain at least 2 panes"
        );
        anyhow::ensure!(
            template.node.remove_at(path),
            "could not remove the selected pane-template node"
        );
        self.selected_node = Some(path.parent().unwrap_or(PaneTemplateNodePath::ROOT));
        Ok(())
    }

    pub fn swap_selected_children(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.selected_is_editable(),
            "read-only pane templates cannot be edited"
        );
        let path = self
            .selected_node
            .context("no pane-template node is selected")?;
        let template = self
            .selected_mut()
            .context("no pane template is selected")?;
        anyhow::ensure!(
            template.node.swap_children(path),
            "the selected node is not a split"
        );
        Ok(())
    }

    pub fn set_selected_axis(&mut self, axis: PaneSplitAxis) -> Result<()> {
        anyhow::ensure!(
            self.selected_is_editable(),
            "read-only pane templates cannot be edited"
        );
        let path = self
            .selected_node
            .context("no pane-template node is selected")?;
        let template = self
            .selected_mut()
            .context("no pane template is selected")?;
        anyhow::ensure!(
            template.node.set_axis(path, axis),
            "the selected node is not a split"
        );
        Ok(())
    }

    pub fn create_empty(&mut self) -> usize {
        self.insert_custom(
            "custom".to_owned(),
            PaneTemplateNodeForm::empty_two_pane(),
            Vec::new(),
        )
    }

    pub fn duplicate_selected(&mut self) -> Result<usize> {
        let selected = self.selected().context("no pane template is selected")?;
        let base = format!("{}-copy", selected.name.text);
        let node = selected.node.clone();
        let environment = selected.environment.clone();
        Ok(self.insert_custom(base, node, environment))
    }

    fn insert_custom(
        &mut self,
        base: String,
        node: PaneTemplateNodeForm,
        environment: Vec<PaneTemplateEnvironmentForm>,
    ) -> usize {
        let name = self.unique_name(&base);
        let index = self.templates.len();
        self.templates.push(PaneTemplateForm {
            name: TextField::new(name.clone()),
            original_name: name,
            overridden: true,
            inherited_source: None,
            environment,
            node,
        });
        self.selected_template = index;
        self.selected_node = Some(PaneTemplateNodePath::ROOT);
        index
    }

    fn unique_name(&self, base: &str) -> String {
        let used = self
            .templates
            .iter()
            .map(|template| template.name.text.as_str())
            .collect::<HashSet<_>>();
        if !used.contains(base) {
            return base.to_owned();
        }
        for number in 2.. {
            let candidate = format!("{base}-{number}");
            if !used.contains(candidate.as_str()) {
                return candidate;
            }
        }
        unreachable!("a finite template list always has an available name")
    }

    pub fn delete_selected(&mut self, referenced: bool) -> Result<()> {
        let index = self.selected_template;
        let template = self
            .templates
            .get(index)
            .context("no pane template is selected")?;
        if template.is_pristine_inherited() {
            anyhow::bail!("read-only pane templates cannot be deleted");
        }
        if referenced && (!template.inherited() || template.name.text != template.original_name) {
            anyhow::bail!("the pane template is still referenced");
        }
        if let Some(inherited) = template.inherited_source.clone() {
            let template = self.templates.get_mut(index).unwrap();
            template.name = TextField::new(template.original_name.clone());
            template.environment = inherited.environment.clone();
            template.node = inherited.node.clone();
            template.overridden = false;
        } else {
            self.templates.remove(index);
            if self.templates.is_empty() {
                self.selected_template = 0;
            } else {
                self.selected_template = index.min(self.templates.len() - 1);
            }
        }
        self.selected_node = Some(PaneTemplateNodePath::ROOT);
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        let mut names = HashSet::new();
        for template in &self.templates {
            anyhow::ensure!(
                !template.name.text.trim().is_empty(),
                "pane template names must not be empty"
            );
            anyhow::ensure!(
                !template.name.text.contains('\0'),
                "pane template names must not contain NUL"
            );
            anyhow::ensure!(
                names.insert(template.name.text.to_ascii_lowercase()),
                "pane template names must be unique"
            );
            if template.is_pristine_inherited() {
                continue;
            }
            validate_pane_template_environment(
                &template.environment,
                &format!("pane template {:?}", template.name.text),
            )?;
            anyhow::ensure!(
                (2..=64).contains(&template.node.pane_count()),
                "pane template {:?} must contain between 2 and 64 panes",
                template.name.text
            );
            template
                .node
                .validate(PaneTemplateNodePath::ROOT, &self.available_profiles)
                .with_context(|| format!("validating pane template {:?}", template.name.text))?;
        }
        Ok(())
    }

    pub fn to_value(&self) -> Result<Value> {
        self.validate()?;
        let mut templates = Map::new();
        for template in &self.templates {
            if template.is_pristine_inherited() {
                continue;
            }
            let mut value = Map::new();
            value.insert("layout".into(), template.node.to_value()?);
            if !template.environment.is_empty() {
                value.insert(
                    "env".into(),
                    pane_template_environment_value(&template.environment),
                );
            }
            templates.insert(template.name.text.clone(), Value::Object(value));
        }
        Ok(Value::Object(templates))
    }

    #[allow(dead_code)]
    pub fn has_custom_values(&self) -> bool {
        self.templates
            .iter()
            .any(|template| !template.is_pristine_inherited())
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
    /// An explicit configuration override. None means automatic inference.
    pub icon: Option<ProfileIcon>,
    pub automatic_icon: ProfileIcon,
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
    pub pane_templates: PaneTemplatesForm,
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
            .map(|resolved| -> Result<ProfileForm> {
                let configured = configured_profiles.iter().find_map(|profile| {
                    let profile = profile.as_object()?;
                    profile
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.eq_ignore_ascii_case(&resolved.name))
                        .then_some(profile)
                });
                let icon = configured
                    .and_then(|profile| profile.get("icon"))
                    .map(ProfileIcon::parse)
                    .transpose()?
                    .flatten();
                let detected = configured.is_none_or(|profile| !profile.contains_key("program"));
                Ok(ProfileForm {
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
                    icon,
                    automatic_icon: ProfileIcon::automatic_for_profile(
                        &resolved.name,
                        &resolved.command,
                    ),
                    hidden: configured
                        .and_then(|profile| profile.get("hidden"))
                        .and_then(Value::as_bool)
                        .unwrap_or_else(|| profile_is_hidden(resolved, &config.hidden_profiles)),
                    detected,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let pane_templates = PaneTemplatesForm::load(root.get("pane_split_templates"), config)?;
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
            pane_templates,
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
                            !profile.detected
                                || profile.theme.is_some()
                                || profile.icon.is_some()
                                || profile.hidden
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
                            if let Some(icon) = &profile.icon
                                && let Some(name) = icon.name()
                            {
                                value.insert("icon".into(), json!(name));
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
        let pane_templates = self.pane_templates.to_value()?;
        if pane_templates
            .as_object()
            .is_some_and(|templates| !templates.is_empty())
        {
            root.insert("pane_split_templates".into(), pane_templates);
        } else {
            root.remove("pane_split_templates");
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
        serde_json::from_str::<Vec<Value>>(include_str!("../keymap.example.json"))
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
