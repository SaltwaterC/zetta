//! The typed form behind the pane-template editor.
//!
//! A template is a recursive tree, and the form is that tree with every field
//! editable and every edit addressed by a `PaneTemplateNodePath` — a bitset of
//! "is this the second child?" rather than a `Vec`, so a control is cheap to
//! clone. The form overlays either the built-in presets (the user
//! configuration) or the resolved user configuration (a project), which is why
//! a template knows whether it is still pristine and inherited.

use super::*;

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
    StackProgram(usize),
    StackArgument(usize, usize),
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
    pub dark_theme: Option<String>,
    pub environment: Vec<PaneTemplateEnvironmentForm>,
    pub overlay: Option<PaneTemplateOverlayForm>,
    /// Commands seeded as stacked entries in this pane.
    pub stack: Vec<PaneTemplateCommandForm>,
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
            dark_theme: None,
            environment: Vec::new(),
            overlay: None,
            stack: Vec::new(),
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

    /// The stacked commands the whole tree declares. Each becomes a terminal, so
    /// they share the pane budget.
    pub fn stacked_command_count(&self) -> usize {
        match self {
            Self::Pane(pane) => pane.stack.len(),
            Self::Split { first, second, .. } => {
                first.stacked_command_count() + second.stacked_command_count()
            }
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
                    PaneTemplateSourceForm::Command(pane_template_command_form(command))
                } else {
                    PaneTemplateSourceForm::Inherit
                },
                theme: pane.theme.clone(),
                dark_theme: pane.dark_theme.clone(),
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
                stack: pane.stack.iter().map(pane_template_command_form).collect(),
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
                anyhow::ensure!(
                    pane.stack.len() < MAX_PANES_PER_TAB,
                    "pane {} must not declare more than {} stacked commands",
                    path_label(path),
                    MAX_PANES_PER_TAB - 1
                );
                for (index, command) in pane.stack.iter().enumerate() {
                    anyhow::ensure!(
                        !command.program.text.trim().is_empty(),
                        "pane {} stacked command {} program is required",
                        path_label(path),
                        index + 1
                    );
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
                        object.insert("command".into(), pane_template_command_value(command));
                    }
                }
                if let Some(theme) = &pane.theme
                    && !theme.is_empty()
                {
                    object.insert("theme".into(), json!(theme));
                }
                if let Some(dark_theme) = &pane.dark_theme
                    && !dark_theme.is_empty()
                {
                    object.insert("dark_theme".into(), json!(dark_theme));
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
                if !pane.stack.is_empty() {
                    object.insert(
                        "stack".into(),
                        Value::Array(
                            pane.stack
                                .iter()
                                .map(pane_template_command_value)
                                .collect::<Vec<_>>(),
                        ),
                    );
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

/// Shared by the leaf's own `command` and by each of its stacked commands,
/// which use the same `{program, args}` shape.
fn pane_template_command_form(command: &PaneSplitCommand) -> PaneTemplateCommandForm {
    PaneTemplateCommandForm {
        program: TextField::new(command.program.clone()),
        args: command.args.iter().cloned().map(TextField::new).collect(),
    }
}

fn pane_template_command_value(command: &PaneTemplateCommandForm) -> Value {
    let mut object = Map::new();
    object.insert("program".into(), json!(command.program.text));
    let args = command
        .args
        .iter()
        .map(|argument| json!(argument.text))
        .collect::<Vec<_>>();
    if !args.is_empty() {
        object.insert("args".into(), Value::Array(args));
    }
    Value::Object(object)
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
                (2..=MAX_PANES_PER_TAB).contains(&template.node.pane_count()),
                "pane template {:?} must contain between 2 and {MAX_PANES_PER_TAB} panes",
                template.name.text
            );
            anyhow::ensure!(
                template.node.pane_count() + template.node.stacked_command_count()
                    <= MAX_PANES_PER_TAB,
                "pane template {:?} must not declare more than {MAX_PANES_PER_TAB} panes and stacked commands combined",
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

#[cfg(test)]
#[path = "../tests/settings_editor/pane_templates.rs"]
mod tests;
