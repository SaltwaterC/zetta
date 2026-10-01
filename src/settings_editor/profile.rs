//! A profile as the settings forms edit it, whichever file it is saved to.
//!
//! The user configuration's profiles and a project's profile overrides are the
//! same thing in the file — an entry under `profiles`, matched to a detected or
//! inherited profile by name — and they used to be two form types with two
//! parsers, two serializers and two sets of checks, which disagreed on
//! trimming and on which names they refused. [`ProfileForm`] is the one model,
//! with one conversion to and from a file entry and one check of a profile
//! list; what differs between the two files (which fields a detected profile
//! keeps, whether a nameless override can be resolved) stays with each form.

use super::*;

#[derive(Clone, Debug)]
pub struct ProfileForm {
    pub name: TextField,
    pub program: TextField,
    /// One field per argument. This used to be one comma-separated field, and
    /// splitting it on save turned `["-c", "echo a,b"]` into three arguments
    /// every time any setting was saved.
    pub arguments: Vec<TextField>,
    pub theme: Option<String>,
    pub dark_theme: Option<String>,
    /// An explicit configuration override. None means automatic inference.
    pub icon: Option<ProfileIcon>,
    /// The icon shown while `icon` is unset.
    pub automatic_icon: ProfileIcon,
    pub hidden: bool,
    /// A profile found on this machine rather than written by the user: its
    /// program and arguments are not the form's to edit.
    pub detected: bool,
}

impl ProfileForm {
    /// A profile with nothing filled in yet, as the Add profile modal and a
    /// new project override start.
    pub(crate) fn blank() -> Self {
        Self {
            name: TextField::default(),
            program: TextField::default(),
            arguments: Vec::new(),
            theme: None,
            dark_theme: None,
            icon: None,
            automatic_icon: ProfileIcon::Zetta,
            hidden: false,
            detected: false,
        }
    }

    /// A profile entry read straight from a file, with no detected profile
    /// behind it: a project's override. Its fields are checked against the
    /// configuration's own file mirror first, so an override accepts exactly
    /// what a user profile does.
    pub(crate) fn from_entry(value: &Value, automatic_icon: ProfileIcon) -> Result<Self> {
        let object = value
            .as_object()
            .context("each profile must be an object")?;
        crate::config::check_profile_fields(value)?;
        let string = |key: &str| object.get(key).and_then(Value::as_str);
        Ok(Self {
            name: TextField::new(string("name").context("profile.name must be a string")?),
            program: TextField::new(string("program").unwrap_or_default()),
            arguments: profile_arguments_from_json(object.get("args")),
            theme: string("theme").map(str::to_owned),
            dark_theme: string("dark_theme").map(str::to_owned),
            icon: object
                .get("icon")
                .map(ProfileIcon::parse)
                .transpose()?
                .flatten(),
            automatic_icon,
            hidden: object
                .get("hidden")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            detected: false,
        })
    }

    /// The profile as a file entry. The name and program are written trimmed;
    /// arguments exactly as typed; and only the overrides that are set.
    pub(crate) fn to_entry(&self) -> Value {
        let mut value = Map::new();
        value.insert("name".into(), json!(self.name.text.trim()));
        let program = self.program.text.trim();
        if !program.is_empty() {
            value.insert("program".into(), json!(program));
            value.insert("args".into(), profile_arguments_to_json(&self.arguments));
        }
        if let Some(theme) = &self.theme {
            value.insert("theme".into(), json!(theme));
        }
        if let Some(dark_theme) = &self.dark_theme {
            value.insert("dark_theme".into(), json!(dark_theme));
        }
        if let Some(name) = self.icon.as_ref().and_then(ProfileIcon::name) {
            value.insert("icon".into(), json!(name));
        }
        if self.hidden {
            value.insert("hidden".into(), json!(true));
        }
        Value::Object(value)
    }

    fn has_arguments(&self) -> bool {
        self.arguments
            .iter()
            .any(|argument| !argument.text.trim().is_empty())
    }
}

/// What is wrong with a list of profiles, naming the profile and the field so
/// a form can take the keyboard there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProfileProblem {
    EmptyName(usize),
    DuplicateName(usize, String),
    ArgumentsWithoutProgram(usize, String),
}

impl ProfileProblem {
    /// The index of the profile at fault.
    pub(crate) fn profile(&self) -> usize {
        match self {
            Self::EmptyName(index)
            | Self::DuplicateName(index, _)
            | Self::ArgumentsWithoutProgram(index, _) => *index,
        }
    }

    /// Whether the fault is in the program field rather than the name.
    pub(crate) fn is_in_program(&self) -> bool {
        matches!(self, Self::ArgumentsWithoutProgram(..))
    }
}

impl std::fmt::Display for ProfileProblem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyName(_) => formatter.write_str("Profile names must not be empty"),
            Self::DuplicateName(_, name) => write!(
                formatter,
                "Two profiles are named {name:?}; profile names must be unique"
            ),
            Self::ArgumentsWithoutProgram(_, name) => write!(
                formatter,
                "Profile {name:?} has arguments but no program to pass them to"
            ),
        }
    }
}

/// The checks every profile list gets, whichever file it is saved to. The file
/// merges a profile into an earlier one of the same name — that is how an
/// entry overrides a detected shell — so two rows sharing a name would
/// silently save as one; and an entry without a program drops its arguments.
pub(crate) fn check_profiles(profiles: &[ProfileForm]) -> Result<(), ProfileProblem> {
    let mut names = HashSet::new();
    for (index, profile) in profiles.iter().enumerate() {
        let name = profile.name.text.trim();
        if name.is_empty() {
            return Err(ProfileProblem::EmptyName(index));
        }
        if !names.insert(name.to_ascii_lowercase()) {
            return Err(ProfileProblem::DuplicateName(index, name.to_owned()));
        }
        if !profile.detected && profile.program.text.trim().is_empty() && profile.has_arguments() {
            return Err(ProfileProblem::ArgumentsWithoutProgram(
                index,
                name.to_owned(),
            ));
        }
    }
    Ok(())
}

/// A profile's `args` as the fields a form edits, one per argument.
pub(crate) fn profile_arguments_from_json(args: Option<&Value>) -> Vec<TextField> {
    args.and_then(Value::as_array)
        .map(|args| {
            args.iter()
                .filter_map(Value::as_str)
                .map(TextField::new)
                .collect()
        })
        .unwrap_or_default()
}

/// The fields back to `args`. Each argument is written exactly as typed — a
/// comma, a space or a quote is part of it — and a field left empty (an
/// argument added but never filled in) is not written at all.
pub(crate) fn profile_arguments_to_json(arguments: &[TextField]) -> Value {
    Value::Array(
        arguments
            .iter()
            .filter(|argument| !argument.text.trim().is_empty())
            .map(|argument| json!(argument.text))
            .collect(),
    )
}

#[cfg(test)]
#[path = "../tests/settings_editor/profile.rs"]
mod tests;
