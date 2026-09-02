use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
};

use anyhow::{Context as _, Result};
use serde_json::Value;

/// The command string, its arguments, and the environment it carries over
/// the local process-control protocol are intentionally bounded. This keeps a
/// project file from turning one control request into an unbounded allocation.
pub(crate) const MAX_SHELL_COMMAND_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RegisteredProjectCommand {
    pub(crate) command: String,
    pub(crate) environment: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProjectCommandInvocation {
    List,
    Run {
        name: String,
        arguments: Vec<String>,
    },
}

pub(crate) fn parse_project_commands(
    value: &Value,
) -> Result<BTreeMap<String, RegisteredProjectCommand>> {
    let object = value
        .as_object()
        .context("commands must be an object of command strings or objects")?;
    object
        .iter()
        .map(|(name, value)| {
            validate_command_name(name)?;
            let (command, environment) = match value {
                Value::String(command) => (command.clone(), BTreeMap::new()),
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
                        .context("project command objects require a string command field")?
                        .to_owned();
                    let environment = object
                        .get("env")
                        .map(parse_command_environment)
                        .transpose()?
                        .unwrap_or_default();
                    (command, environment)
                }
                _ => anyhow::bail!(
                    "project command {name:?} must be a string or an object with command and env"
                ),
            };
            validate_command_string(&command)?;
            Ok((
                name.clone(),
                RegisteredProjectCommand {
                    command,
                    environment,
                },
            ))
        })
        .collect()
}

pub(crate) fn validate_command_name(name: &str) -> Result<()> {
    anyhow::ensure!(!name.is_empty(), "project command names must not be empty");
    anyhow::ensure!(
        !name.starts_with('-'),
        "project command names must not begin with '-': {name:?}"
    );
    anyhow::ensure!(
        !name
            .chars()
            .any(|character| character.is_whitespace() || character.is_control()),
        "project command names must not contain whitespace or control characters: {name:?}"
    );
    anyhow::ensure!(
        name != "--list",
        "project command name '--list' is reserved for listing commands"
    );
    Ok(())
}

pub(crate) fn validate_command_string(command: &str) -> Result<()> {
    anyhow::ensure!(
        !command.trim().is_empty(),
        "project command strings must not be empty"
    );
    anyhow::ensure!(
        !command.contains('\0'),
        "project command strings must not contain NUL"
    );
    anyhow::ensure!(
        command.len() <= MAX_SHELL_COMMAND_BYTES,
        "project command exceeds the {} KiB limit",
        MAX_SHELL_COMMAND_BYTES / 1024
    );
    Ok(())
}

pub(crate) fn validate_environment_entry(name: &str, value: &str, description: &str) -> Result<()> {
    anyhow::ensure!(
        !name.is_empty() && !name.contains(['=', '\0']),
        "{description} variable names must not be empty or contain '=' or NUL"
    );
    anyhow::ensure!(
        !name
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("ZETTA_")),
        "{description} variables may not override reserved ZETTA_* variables"
    );
    anyhow::ensure!(
        !value.contains('\0'),
        "{description} values must not contain NUL"
    );
    Ok(())
}

pub(crate) fn validate_command_environment_entry(
    name: &str,
    value: &str,
    description: &str,
) -> Result<()> {
    validate_environment_entry(name, value, description)?;
    let mut characters = name.bytes();
    anyhow::ensure!(
        characters
            .next()
            .is_some_and(|character| character.is_ascii_alphabetic() || character == b'_')
            && characters.all(|character| character.is_ascii_alphanumeric() || character == b'_'),
        "{description} variable names must be shell identifiers (letters, digits, and underscores; the first character cannot be a digit): {name:?}"
    );
    Ok(())
}

pub(crate) fn parse_command_environment(value: &Value) -> Result<BTreeMap<String, String>> {
    let object = value
        .as_object()
        .context("project command env must be an object of strings")?;
    let mut names = HashSet::new();
    object
        .iter()
        .map(|(name, value)| {
            let value = value
                .as_str()
                .context("project command environment values must be strings")?;
            validate_command_environment_entry(name, value, "project command environment")?;
            anyhow::ensure!(
                names.insert(name.to_ascii_uppercase()),
                "duplicate project command environment variable {name:?}"
            );
            Ok((name.clone(), value.to_owned()))
        })
        .collect()
}

pub(crate) fn merge_command_environment(
    project_environment: &HashMap<String, String>,
    command_environment: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut environment = project_environment
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    for (name, value) in command_environment {
        // Environment names are case-insensitive on Windows, and the project
        // form treats them that way on every platform so a configuration does
        // not behave differently merely because it moved between machines.
        environment.retain(|project_name, _| !project_name.eq_ignore_ascii_case(name));
        environment.insert(name.clone(), value.clone());
    }
    environment
}

pub(crate) fn parse_project_command_args(
    arguments: &[OsString],
) -> Result<ProjectCommandInvocation> {
    let delimiter = arguments.iter().position(|argument| argument == "--");
    let options = &arguments[..delimiter.unwrap_or(arguments.len())];
    if options
        .iter()
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        println!("{}", crate::startup::command_help());
        std::process::exit(0);
    }

    let name = arguments
        .first()
        .context("zetta cmd requires a command name or --list; run zetta cmd --help for usage")?
        .to_string_lossy()
        .into_owned();
    if name == "--list" || name == "-l" {
        anyhow::ensure!(
            arguments.len() == 1,
            "zetta cmd --list does not accept arguments"
        );
        return Ok(ProjectCommandInvocation::List);
    }
    anyhow::ensure!(
        !name.starts_with('-'),
        "unknown zetta cmd option {name:?}; run zetta cmd --help for usage"
    );
    if let Some(delimiter) = delimiter {
        anyhow::ensure!(delimiter == 1, "arguments for zetta cmd must follow '--'");
    }
    validate_command_name(&name)?;

    let arguments = match delimiter {
        Some(index) => arguments[index + 1..]
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect(),
        None => {
            anyhow::ensure!(
                arguments.len() == 1,
                "arguments for zetta cmd must follow '--'"
            );
            Vec::new()
        }
    };
    Ok(ProjectCommandInvocation::Run { name, arguments })
}

#[cfg(test)]
#[path = "tests/project_commands.rs"]
mod tests;
