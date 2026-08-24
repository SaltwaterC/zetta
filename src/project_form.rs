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
use crate::settings_editor::{PaneTemplatesForm, TextField};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectTextField {
    WorkingDirectory,
    EnvironmentName(usize),
    EnvironmentValue(usize),
    ProfileName(usize),
    ProfileProgram(usize),
    ProfileArguments(usize),
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectEnvironmentForm {
    pub(crate) name: TextField,
    pub(crate) value: TextField,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectProfileForm {
    pub(crate) name: TextField,
    pub(crate) program: TextField,
    pub(crate) arguments: TextField,
    pub(crate) theme: Option<String>,
    pub(crate) dark_theme: Option<String>,
    /// An explicit icon override. `None` means the profile keeps whichever icon
    /// the user configuration infers for it.
    pub(crate) icon: Option<ProfileIcon>,
    pub(crate) hidden: bool,
}

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
        let profiles = object
            .get("profiles")
            .map(|value| {
                value
                    .as_array()
                    .context("profiles must be an array")?
                    .iter()
                    .map(parse_profile_form)
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
            ProjectTextField::ProfileName(index) => self
                .profiles
                .get_mut(index)
                .map(|profile| &mut profile.name),
            ProjectTextField::ProfileProgram(index) => self
                .profiles
                .get_mut(index)
                .map(|profile| &mut profile.program),
            ProjectTextField::ProfileArguments(index) => self
                .profiles
                .get_mut(index)
                .map(|profile| &mut profile.arguments),
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
            anyhow::ensure!(
                !name.is_empty() && !name.contains(['=', '\0']),
                "environment variable names must not be empty or contain '='"
            );
            anyhow::ensure!(
                !name
                    .get(..6)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("ZETTA_")),
                "environment variables may not override reserved ZETTA_* variables"
            );
            anyhow::ensure!(
                !entry.value.text.contains('\0'),
                "environment values must not contain NUL"
            );
            anyhow::ensure!(
                names.insert(name.to_ascii_uppercase()),
                "duplicate environment variable {name:?}"
            );
        }

        let mut profile_names = HashSet::new();
        for profile in &self.profiles {
            let name = profile.name.text.trim();
            anyhow::ensure!(!name.is_empty(), "profile names must not be empty");
            anyhow::ensure!(
                profile_names.insert(name.to_ascii_lowercase()),
                "duplicate profile {name:?}"
            );
            anyhow::ensure!(
                !profile.program.text.trim().is_empty()
                    || arguments(&profile.arguments.text).is_empty(),
                "profile {name:?} needs a program before it can take arguments"
            );
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
                Value::Array(self.profiles.iter().map(profile_value).collect()),
            );
        }
        serde_json::to_string_pretty(&Value::Object(root))
            .context("serializing the project configuration")
    }
}

/// Splits a comma-separated argument field the way the configuration page does,
/// so both forms accept the same input.
pub(crate) fn arguments(text: &str) -> Vec<&str> {
    text.split(',')
        .map(str::trim)
        .filter(|argument| !argument.is_empty())
        .collect()
}

fn non_empty(text: &str) -> Option<&str> {
    Some(text.trim()).filter(|text| !text.is_empty())
}

fn parse_profile_form(value: &Value) -> Result<ProjectProfileForm> {
    let object = value
        .as_object()
        .context("each profile must be an object")?;
    const FIELDS: &[&str] = &[
        "name",
        "program",
        "args",
        "theme",
        "dark_theme",
        "icon",
        "hidden",
    ];
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        anyhow::bail!("unrecognized profile field {field:?}");
    }
    Ok(ProjectProfileForm {
        name: TextField::new(
            object
                .get("name")
                .and_then(Value::as_str)
                .context("profile.name must be a string")?,
        ),
        program: TextField::new(
            object
                .get("program")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        arguments: TextField::new(
            object
                .get("args")
                .and_then(Value::as_array)
                .map(|args| {
                    args.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default(),
        ),
        theme: object
            .get("theme")
            .and_then(Value::as_str)
            .map(str::to_owned),
        dark_theme: object
            .get("dark_theme")
            .and_then(Value::as_str)
            .map(str::to_owned),
        icon: object
            .get("icon")
            .map(ProfileIcon::parse)
            .transpose()?
            .flatten(),
        hidden: object
            .get("hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn profile_value(profile: &ProjectProfileForm) -> Value {
    let mut value = Map::new();
    value.insert("name".into(), json!(profile.name.text.trim()));
    if let Some(program) = non_empty(&profile.program.text) {
        value.insert("program".into(), json!(program));
        value.insert(
            "args".into(),
            Value::Array(
                arguments(&profile.arguments.text)
                    .into_iter()
                    .map(|argument| json!(argument))
                    .collect(),
            ),
        );
    }
    if let Some(theme) = profile.theme.as_deref() {
        value.insert("theme".into(), json!(theme));
    }
    if let Some(dark_theme) = profile.dark_theme.as_deref() {
        value.insert("dark_theme".into(), json!(dark_theme));
    }
    if let Some(name) = profile.icon.as_ref().and_then(ProfileIcon::name) {
        value.insert("icon".into(), json!(name));
    }
    if profile.hidden {
        value.insert("hidden".into(), json!(true));
    }
    Value::Object(value)
}

/// Writes `text` to the project's configuration file after checking that it
/// still parses as a project overlay, so a form bug can never replace a working
/// file with one Zetta would refuse to load.
pub(crate) fn save(root: &Path, base: &Config, text: &str) -> Result<PathBuf> {
    ProjectConfig::parse(text, root, base)?;
    let path = ProjectConfig::path_for(root);
    crate::project::write_text_atomically(&path, text)?;
    Ok(path)
}

#[cfg(test)]
#[path = "tests/project_form.rs"]
mod tests;
