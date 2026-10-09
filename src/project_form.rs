//! The typed form behind the settings Projects tab's project configuration
//! builder, and its serialization back to `.zetta/config.json`.
//!
//! A project file is an overlay: a field that is absent inherits the user
//! configuration, so every control here has to be able to express "not set"
//! rather than defaulting to a concrete value the way [`ConfigurationForm`]
//! does. Authoritative validation still belongs to
//! [`ProjectConfig::parse`](crate::project::ProjectConfig::parse), which the
//! save path runs against the serialized text before replacing the file; the
//! checks in [`ProjectForm::validate`] exist to report the same problems
//! without touching the filesystem while the form is being edited.
//!
//! [`ConfigurationForm`]: crate::settings_editor::ConfigurationForm

use std::{
    collections::HashSet,
    fs, io,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context as _, Result};
use serde_json::{Map, Value, json};
use ui::IconName;

use crate::config::Config;
use crate::profile_icon::ProfileIcon;
use crate::project::{ProjectConfig, validate_project_fields};
use crate::project_commands::{
    parse_command_environment, validate_command_environment_entry, validate_command_name,
    validate_command_string, validate_environment_entry,
};
use crate::settings_editor::{PaneTemplatesForm, ProfileForm};
use crate::text_edit::TextField;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectTextField {
    WorkingDirectory,
    EnvironmentName(usize),
    EnvironmentValue(usize),
    CommandName(usize),
    Command(usize),
    CommandEnvironmentName(usize, usize),
    CommandEnvironmentValue(usize, usize),
    ProfileName(usize),
    ProfileProgram(usize),
    ProfileArgument(usize, usize),
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectEnvironmentForm {
    pub(crate) name: TextField,
    pub(crate) value: TextField,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectCommandForm {
    pub(crate) name: TextField,
    pub(crate) command: TextField,
    pub(crate) environment: Vec<ProjectEnvironmentForm>,
    /// Preserve the object form when it has no nested environment entries.
    pub(crate) object: bool,
}

/// A project's profile override: the same form the user configuration's
/// profiles use, since it is the same entry in the file.
pub(crate) type ProjectProfileForm = crate::settings_editor::ProfileForm;

/// A project's `default_tab_icon`, which is three-valued: absent inherits the
/// user configuration, `null` means new tabs get no icon, and a name selects
/// one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectTabIcon {
    Inherit,
    None,
    Icon(IconName),
}

impl ProjectTabIcon {
    pub(crate) fn label(self) -> String {
        match self {
            Self::Inherit => "Inherit".to_owned(),
            Self::None => "No icon".to_owned(),
            Self::Icon(icon) => crate::tab_icon_picker::tab_icon_label(icon),
        }
    }

    pub(crate) fn icon(self) -> Option<IconName> {
        match self {
            Self::Icon(icon) => Some(icon),
            Self::Inherit | Self::None => None,
        }
    }
}

/// The label the theme, profile, and split dropdowns use for "leave this to the
/// user configuration". Kept in one place because the dropdown machinery
/// round-trips selections through their display strings.
pub(crate) const PROJECT_INHERIT_LABEL: &str = "Inherit";

#[derive(Clone, Debug)]
pub(crate) struct ProjectForm {
    pub(crate) theme: Option<String>,
    pub(crate) dark_theme: Option<String>,
    /// A project-relative directory. Empty means the project root.
    pub(crate) working_directory: TextField,
    pub(crate) default_profile: Option<String>,
    pub(crate) default_tab_icon: ProjectTabIcon,
    pub(crate) inactive_pane_opacity: Option<f32>,
    pub(crate) environment: Vec<ProjectEnvironmentForm>,
    pub(crate) commands: Vec<ProjectCommandForm>,
    pub(crate) initial_split: Option<String>,
    pub(crate) profiles: Vec<ProjectProfileForm>,
    pub(crate) pane_templates: PaneTemplatesForm,
    /// The user configuration's profile names. A profile row without a program
    /// is an override matched against these by name.
    pub(crate) inherited_profiles: Vec<String>,
}

impl ProjectForm {
    pub(crate) fn load(root: &Path, base: &Config) -> Result<Self> {
        let path = ProjectConfig::path_for(root);
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading project configuration {}", path.display()));
            }
        };
        Self::parse(&source, &path, base)
    }

    pub(crate) fn parse(source: &str, path: &Path, base: &Config) -> Result<Self> {
        let value: Value = if source.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(source)
                .with_context(|| format!("parsing project configuration {}", path.display()))?
        };
        let object = value
            .as_object()
            .context("project configuration root must be an object")?;
        validate_project_fields(object)?;

        // `env` and `initial_split` are project-only fields, so the resolved
        // view of the file is everything else applied over the user
        // configuration. Working directories are resolved against the
        // filesystem when the project is actually loaded, not here, so opening
        // the builder never fails on a directory that has since been moved.
        let mut overlay = object.clone();
        overlay.remove("env");
        overlay.remove("commands");
        overlay.remove("initial_split");
        let effective = Config::parse_overlay(
            &serde_json::to_string(&Value::Object(overlay))?,
            base.clone(),
            path,
        )?;
        let pane_templates =
            PaneTemplatesForm::load_overlay(object.get("pane_split_templates"), base, &effective)?;

        let string = |field: &str| object.get(field).and_then(Value::as_str).map(str::to_owned);
        let default_tab_icon = match object.get("default_tab_icon") {
            None => ProjectTabIcon::Inherit,
            Some(Value::Null) => ProjectTabIcon::None,
            Some(value) => {
                let name = value
                    .as_str()
                    .context("default_tab_icon must be an icon name or null")?;
                ProjectTabIcon::Icon(name.parse().map_err(|_| {
                    anyhow::anyhow!("default_tab_icon must be a built-in icon name, got {name:?}")
                })?)
            }
        };
        let inactive_pane_opacity = object
            .get("inactive_pane_opacity")
            .map(|value| {
                let opacity = value
                    .as_f64()
                    .context("inactive_pane_opacity must be a number")?;
                anyhow::ensure!(
                    (0. ..=1.).contains(&opacity),
                    "inactive_pane_opacity must be between 0 and 1"
                );
                Ok(opacity as f32)
            })
            .transpose()?;
        let mut environment = object
            .get("env")
            .map(|value| {
                value
                    .as_object()
                    .context("env must be an object of strings")
                    .map(|entries| {
                        entries
                            .iter()
                            .map(|(name, value)| ProjectEnvironmentForm {
                                name: TextField::new(name),
                                value: TextField::new(
                                    value.as_str().map(str::to_owned).unwrap_or_default(),
                                ),
                            })
                            .collect::<Vec<_>>()
                    })
            })
            .transpose()?
            .unwrap_or_default();
        environment.sort_by(|left, right| left.name.text.cmp(&right.name.text));
        let mut commands = match object.get("commands") {
            Some(value) => {
                let entries = value
                    .as_object()
                    .context("commands must be an object of command strings or objects")?;
                entries
                    .iter()
                    .map(|(name, value)| parse_command_form(name, value))
                    .collect::<Result<Vec<_>>>()?
            }
            None => Vec::new(),
        };
        commands.sort_by(|left, right| left.name.text.cmp(&right.name.text));
        let profiles = object
            .get("profiles")
            .map(|value| {
                value
                    .as_array()
                    .context("profiles must be an array")?
                    .iter()
                    .map(|value| {
                        ProfileForm::from_entry(value, inherited_profile_icon(base, value))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();

        let mut inherited_profiles = base
            .profiles
            .iter()
            .map(|profile| profile.name.clone())
            .collect::<Vec<_>>();
        inherited_profiles.sort_by_key(|name| name.to_lowercase());
        inherited_profiles.dedup();

        Ok(Self {
            theme: string("theme"),
            dark_theme: string("dark_theme"),
            working_directory: TextField::new(string("working_directory").unwrap_or_default()),
            default_profile: string("default_profile"),
            default_tab_icon,
            inactive_pane_opacity,
            environment,
            commands,
            initial_split: string("initial_split"),
            profiles,
            pane_templates,
            inherited_profiles,
        })
    }

    pub(crate) fn text_mut(&mut self, field: ProjectTextField) -> Option<&mut TextField> {
        match field {
            ProjectTextField::WorkingDirectory => Some(&mut self.working_directory),
            ProjectTextField::EnvironmentName(index) => {
                self.environment.get_mut(index).map(|entry| &mut entry.name)
            }
            ProjectTextField::EnvironmentValue(index) => self
                .environment
                .get_mut(index)
                .map(|entry| &mut entry.value),
            ProjectTextField::CommandName(index) => self
                .commands
                .get_mut(index)
                .map(|command| &mut command.name),
            ProjectTextField::Command(index) => self
                .commands
                .get_mut(index)
                .map(|command| &mut command.command),
            ProjectTextField::CommandEnvironmentName(command_index, environment_index) => self
                .commands
                .get_mut(command_index)
                .and_then(|command| command.environment.get_mut(environment_index))
                .map(|entry| &mut entry.name),
            ProjectTextField::CommandEnvironmentValue(command_index, environment_index) => self
                .commands
                .get_mut(command_index)
                .and_then(|command| command.environment.get_mut(environment_index))
                .map(|entry| &mut entry.value),
            ProjectTextField::ProfileName(index) => self
                .profiles
                .get_mut(index)
                .map(|profile| &mut profile.name),
            ProjectTextField::ProfileProgram(index) => self
                .profiles
                .get_mut(index)
                .map(|profile| &mut profile.program),
            ProjectTextField::ProfileArgument(index, argument) => self
                .profiles
                .get_mut(index)
                .and_then(|profile| profile.arguments.get_mut(argument)),
        }
    }

    /// Every pane template the project can name in `initial_split`: the
    /// inherited ones plus its own.
    pub(crate) fn template_names(&self) -> Vec<String> {
        let mut names = self.pane_templates.names();
        names.retain(|name| !name.trim().is_empty());
        names.sort_by_key(|name| name.to_lowercase());
        names.dedup();
        names
    }

    /// `initial_split` as it should be written out: renaming the template it
    /// names in the same session moves the reference with it, the way a
    /// keybinding follows a renamed user template.
    pub(crate) fn resolved_initial_split(&self) -> Option<&str> {
        let name = self.initial_split.as_deref()?;
        Some(self.pane_templates.current_name_for(name).unwrap_or(name))
    }

    /// Profile names `default_profile` may name: the user configuration's, plus
    /// the ones the form is currently adding.
    pub(crate) fn profile_options(&self) -> Vec<String> {
        let mut names = self.inherited_profiles.clone();
        names.extend(
            self.profiles
                .iter()
                .map(|profile| profile.name.text.clone())
                .filter(|name| !name.trim().is_empty()),
        );
        names.sort_by_key(|name| name.to_lowercase());
        names.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        names
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if let Some(directory) = non_empty(&self.working_directory.text) {
            let relative = Path::new(directory);
            anyhow::ensure!(
                !relative.is_absolute()
                    && relative.components().all(|component| !matches!(
                        component,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )),
                "the working directory must be a project-relative path inside the project"
            );
        }

        let mut names = HashSet::new();
        for entry in &self.environment {
            let name = entry.name.text.as_str();
            validate_environment_entry(name, &entry.value.text, "environment")?;
            anyhow::ensure!(
                names.insert(name.to_ascii_uppercase()),
                "duplicate environment variable {name:?}"
            );
        }

        let mut command_names = HashSet::new();
        for command in &self.commands {
            let name = command.name.text.trim();
            validate_command_name(name)?;
            anyhow::ensure!(
                command_names.insert(name.to_owned()),
                "duplicate project command {name:?}"
            );
            validate_command_string(&command.command.text)?;

            let mut environment_names = HashSet::new();
            for entry in &command.environment {
                validate_command_environment_entry(
                    &entry.name.text,
                    &entry.value.text,
                    "project command environment",
                )?;
                anyhow::ensure!(
                    environment_names.insert(entry.name.text.to_ascii_uppercase()),
                    "duplicate project command environment variable {:?}",
                    entry.name.text
                );
            }
        }

        crate::settings_editor::check_profiles(&self.profiles)
            .map_err(|problem| anyhow::anyhow!("{problem}"))?;
        for profile in &self.profiles {
            let name = profile.name.text.trim();
            // A row without a program is an override of an application profile,
            // matched by name; naming one that does not exist would otherwise
            // only fail when the file is loaded back.
            anyhow::ensure!(
                !profile.program.text.trim().is_empty()
                    || self
                        .inherited_profiles
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(name)),
                "profile {name:?} needs a program, because no application profile has that name"
            );
        }

        if let Some(name) = self.resolved_initial_split() {
            anyhow::ensure!(
                self.template_names()
                    .iter()
                    .any(|candidate| candidate == name),
                "the initial split {name:?} is not an available pane template"
            );
        }

        self.pane_templates.validate()
    }

    fn command_entries(&self) -> Map<String, Value> {
        let mut entries = self.commands.iter().collect::<Vec<_>>();
        entries.sort_by_key(|command| command.name.text.trim().to_owned());
        entries
            .into_iter()
            .map(|command| (command.name.text.trim().to_owned(), command_value(command)))
            .collect()
    }

    /// The editor's command snapshot, independently of validation of other
    /// fields: opening a configuration to repair it must remain possible.
    pub(crate) fn command_fingerprint(&self) -> Result<String> {
        let source = serde_json::to_string(&json!({"commands": self.command_entries()}))?;
        Ok(crate::project_trust::command_approval(&source)?.fingerprint)
    }

    pub(crate) fn to_json(&self) -> Result<String> {
        self.validate()?;
        let mut root = Map::new();
        if let Some(theme) = self.theme.as_deref() {
            root.insert("theme".into(), json!(theme));
        }
        if let Some(dark_theme) = self.dark_theme.as_deref() {
            root.insert("dark_theme".into(), json!(dark_theme));
        }
        if let Some(directory) = non_empty(&self.working_directory.text) {
            root.insert("working_directory".into(), json!(directory));
        }
        if let Some(profile) = self.default_profile.as_deref() {
            root.insert("default_profile".into(), json!(profile));
        }
        match self.default_tab_icon {
            ProjectTabIcon::Inherit => {}
            ProjectTabIcon::None => {
                root.insert("default_tab_icon".into(), Value::Null);
            }
            ProjectTabIcon::Icon(icon) => {
                let name: &'static str = icon.into();
                root.insert("default_tab_icon".into(), json!(name));
            }
        }
        if let Some(opacity) = self.inactive_pane_opacity {
            let opacity = format!("{opacity:.2}")
                .parse::<f64>()
                .context("formatting the inactive pane opacity")?;
            root.insert("inactive_pane_opacity".into(), json!(opacity));
        }
        if !self.environment.is_empty() {
            root.insert(
                "env".into(),
                Value::Object(
                    self.environment
                        .iter()
                        .map(|entry| (entry.name.text.clone(), json!(entry.value.text)))
                        .collect(),
                ),
            );
        }
        if !self.commands.is_empty() {
            root.insert("commands".into(), Value::Object(self.command_entries()));
        }
        if let Some(name) = self.resolved_initial_split() {
            root.insert("initial_split".into(), json!(name));
        }
        let templates = self.pane_templates.to_value()?;
        if templates
            .as_object()
            .is_some_and(|templates| !templates.is_empty())
        {
            root.insert("pane_split_templates".into(), templates);
        }
        if !self.profiles.is_empty() {
            root.insert(
                "profiles".into(),
                Value::Array(self.profiles.iter().map(ProfileForm::to_entry).collect()),
            );
        }
        serde_json::to_string_pretty(&Value::Object(root))
            .context("serializing the project configuration")
    }
}

fn non_empty(text: &str) -> Option<&str> {
    Some(text.trim()).filter(|text| !text.is_empty())
}

fn parse_command_form(name: &str, value: &Value) -> Result<ProjectCommandForm> {
    validate_command_name(name)?;
    match value {
        Value::String(command) => {
            validate_command_string(command)?;
            Ok(ProjectCommandForm {
                name: TextField::new(name),
                command: TextField::new(command),
                environment: Vec::new(),
                object: false,
            })
        }
        Value::Object(object) => {
            if let Some(field) = object
                .keys()
                .find(|field| !matches!(field.as_str(), "command" | "env"))
            {
                anyhow::bail!("unrecognized project command field {field:?}");
            }
            let command = object
                .get("command")
                .and_then(Value::as_str)
                .context("project command objects require a string command field")?;
            let environment = object
                .get("env")
                .map(parse_command_environment)
                .transpose()?
                .unwrap_or_default()
                .into_iter()
                .map(|(name, value)| ProjectEnvironmentForm {
                    name: TextField::new(name),
                    value: TextField::new(value),
                })
                .collect();
            validate_command_string(command)?;
            Ok(ProjectCommandForm {
                name: TextField::new(name),
                command: TextField::new(command),
                environment,
                object: true,
            })
        }
        _ => anyhow::bail!(
            "project command {name:?} must be a string or an object with command and env"
        ),
    }
}

fn command_value(command: &ProjectCommandForm) -> Value {
    if !command.object && command.environment.is_empty() {
        return json!(command.command.text);
    }
    let mut object = Map::new();
    object.insert("command".into(), json!(command.command.text));
    if !command.environment.is_empty() {
        let mut environment = command.environment.iter().collect::<Vec<_>>();
        environment.sort_by_key(|entry| entry.name.text.clone());
        object.insert(
            "env".into(),
            Value::Object(
                environment
                    .iter()
                    .map(|entry| (entry.name.text.clone(), json!(entry.value.text)))
                    .collect(),
            ),
        );
    }
    Value::Object(object)
}

/// The icon an override shows while it sets none of its own: the inherited
/// profile's of that name, or the one its program suggests.
fn inherited_profile_icon(base: &Config, value: &Value) -> ProfileIcon {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let program = value
        .get("program")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !program.trim().is_empty() {
        return ProfileIcon::automatic_for_program(program);
    }
    base.profiles
        .iter()
        .find(|profile| profile.name.eq_ignore_ascii_case(name))
        .map_or(ProfileIcon::Zetta, |profile| {
            ProfileIcon::automatic_for_profile(&profile.name, &profile.command)
        })
}

/// Writes `text` to the project's configuration file after checking that it
/// still parses as a project overlay, so a form bug can never replace a working
/// file with one Zetta would refuse to load.
pub(crate) fn save(root: &Path, base: &Config, text: &str) -> Result<PathBuf> {
    ProjectConfig::parse(text, root, base)?;
    let path = ProjectConfig::path_for(root);
    crate::file_replace::replace_file(&path, text)?;
    Ok(path)
}

#[cfg(test)]
#[path = "tests/project_form.rs"]
mod tests;
