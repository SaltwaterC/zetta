use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};
use gpui::AssetSource as _;
use serde_json::{Map, Value, json};

use crate::{
    config::{Config, Profile, themes_dir},
    process_control::request_existing_process_configuration_reload,
    zetta_assets::ZettaAssets,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProfileCommand {
    List,
    Themes,
    Disable {
        profile: String,
    },
    Enable {
        profile: String,
    },
    Theme {
        profile: String,
        theme: Option<String>,
    },
    Default {
        profile: String,
    },
    Add {
        name: String,
        program: String,
        args: Vec<String>,
        theme: Option<String>,
    },
    Remove {
        profile: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParsedProfileCommand {
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) command: ProfileCommand,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProfileCommandResult {
    pub(crate) changed: bool,
    pub(crate) config_path: PathBuf,
}

pub(crate) fn parse_profile_args(
    arguments: &[std::ffi::OsString],
    initial_config_path: Option<PathBuf>,
) -> Result<ParsedProfileCommand> {
    let mut config_path = initial_config_path;
    let mut command_arguments = Vec::with_capacity(arguments.len());
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--config" | "-c" => {
                anyhow::ensure!(
                    config_path.is_none(),
                    "--config may only be specified once for zetta profile"
                );
                config_path = Some(arguments.next().context("--config requires a path")?.into());
            }
            _ => command_arguments.push(argument.to_string_lossy().into_owned()),
        }
    }

    if command_arguments
        .iter()
        .any(|argument| matches!(argument.as_str(), "--help" | "-h"))
    {
        let operation = command_arguments
            .iter()
            .find(|argument| !argument.starts_with('-'))
            .map(String::as_str);
        println!("{}", profile_operation_help(operation));
        std::process::exit(0);
    }

    let operation = command_arguments
        .first()
        .context("zetta profile requires an operation; run `zetta profile --help` for usage")?;
    let arguments = &command_arguments[1..];
    let command = match operation.as_str() {
        "list" => parse_no_arguments("profile list", arguments, ProfileCommand::List)?,
        "themes" => parse_no_arguments("profile themes", arguments, ProfileCommand::Themes)?,
        "disable" => ProfileCommand::Disable {
            profile: parse_one_profile_argument("profile disable", arguments)?,
        },
        "enable" => ProfileCommand::Enable {
            profile: parse_one_profile_argument("profile enable", arguments)?,
        },
        "theme" => parse_theme_command(arguments)?,
        "default" => ProfileCommand::Default {
            profile: parse_one_profile_argument("profile default", arguments)?,
        },
        "add" => parse_add_command(arguments)?,
        "remove" => ProfileCommand::Remove {
            profile: parse_one_profile_argument("profile remove", arguments)?,
        },
        unknown => anyhow::bail!(
            "unknown zetta profile operation {unknown:?}; run `zetta profile --help` for usage"
        ),
    };

    Ok(ParsedProfileCommand {
        config_path,
        command,
    })
}

fn parse_no_arguments(
    usage: &str,
    arguments: &[String],
    command: ProfileCommand,
) -> Result<ProfileCommand> {
    anyhow::ensure!(
        arguments.is_empty(),
        "{usage} does not accept arguments; run `zetta {usage} --help` for usage"
    );
    Ok(command)
}

fn parse_one_profile_argument(usage: &str, arguments: &[String]) -> Result<String> {
    anyhow::ensure!(
        arguments.len() == 1 && !arguments[0].starts_with('-'),
        "usage: zetta {usage} PROFILE"
    );
    validate_profile_argument(&arguments[0])
}

fn parse_theme_command(arguments: &[String]) -> Result<ProfileCommand> {
    let mut reset = false;
    let mut positional = Vec::new();
    for argument in arguments {
        match argument.as_str() {
            "--reset" | "-r" => {
                anyhow::ensure!(!reset, "--reset may only be specified once");
                reset = true;
            }
            value if value.starts_with('-') => {
                anyhow::bail!("unknown zetta profile theme option {value:?}")
            }
            value => positional.push(value.to_owned()),
        }
    }
    anyhow::ensure!(
        (reset && positional.len() == 1) || (!reset && positional.len() == 2),
        "usage: zetta profile theme PROFILE THEME\n       zetta profile theme PROFILE --reset"
    );
    let profile = validate_profile_argument(&positional[0])?;
    let theme = if reset {
        None
    } else {
        let theme = positional[1].clone();
        anyhow::ensure!(!theme.is_empty(), "profile theme requires a theme name");
        Some(theme)
    };
    Ok(ProfileCommand::Theme { profile, theme })
}

fn parse_add_command(arguments: &[String]) -> Result<ProfileCommand> {
    let mut name = None;
    let mut program = None;
    let mut args = Vec::new();
    let mut theme = None;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--program" | "-p" => {
                anyhow::ensure!(program.is_none(), "--program may only be specified once");
                program = Some(
                    arguments
                        .next()
                        .context("--program requires an executable")?
                        .clone(),
                );
            }
            "--arg" | "-a" => {
                args.push(arguments.next().context("--arg requires a value")?.clone())
            }
            "--theme" | "-t" => {
                anyhow::ensure!(theme.is_none(), "--theme may only be specified once");
                theme = Some(
                    arguments
                        .next()
                        .context("--theme requires a theme name")?
                        .clone(),
                );
            }
            value if value.starts_with('-') => {
                anyhow::bail!("unknown zetta profile add option {value:?}")
            }
            value => {
                anyhow::ensure!(name.is_none(), "profile add accepts only one profile name");
                name = Some(value.to_owned());
            }
        }
    }
    let name = validate_profile_argument(
        &name.context("profile add requires a profile name; run `zetta profile add --help`")?,
    )?;
    let program = program
        .context("profile add requires --program PROGRAM; run `zetta profile add --help`")?;
    anyhow::ensure!(
        !program.trim().is_empty(),
        "profile add requires a non-empty program"
    );
    Ok(ProfileCommand::Add {
        name,
        program,
        args,
        theme,
    })
}

fn validate_profile_argument(profile: &str) -> Result<String> {
    anyhow::ensure!(
        !profile.trim().is_empty() && !profile.contains(['\r', '\n']),
        "profile names must not be empty or contain newlines"
    );
    Ok(profile.to_owned())
}

pub(crate) fn run(
    command: ProfileCommand,
    selected_config_path: Option<&Path>,
) -> Result<ProfileCommandResult> {
    if matches!(command, ProfileCommand::Themes) {
        for theme in profile_theme_names()? {
            println!("{theme}");
        }
        return Ok(ProfileCommandResult {
            changed: false,
            config_path: Config::defaults(selected_config_path, None).config_path,
        });
    }

    let config_path = Config::defaults(selected_config_path, None).config_path;
    let source = read_config_source(&config_path)?;
    let current = Config::parse(&source, Some(&config_path), None)?;

    if command == ProfileCommand::List {
        for profile in current.profiles {
            println!("{}", profile.name);
        }
        return Ok(ProfileCommandResult {
            changed: false,
            config_path,
        });
    }

    let mut root = serde_json::from_str::<Value>(&source)
        .with_context(|| format!("parsing {}", config_path.display()))?
        .as_object()
        .cloned()
        .context("configuration root must be an object")?;

    let changed = apply_mutation(&mut root, &current, &command, &config_path)?;
    if !changed {
        return Ok(ProfileCommandResult {
            changed: false,
            config_path,
        });
    }

    let candidate = serde_json::to_string_pretty(&Value::Object(root))
        .context("serializing profile configuration")?;
    Config::parse(&candidate, Some(&config_path), None)
        .with_context(|| format!("validating {}", config_path.display()))?;
    save_config(&config_path, &candidate)?;

    Ok(ProfileCommandResult {
        changed: true,
        config_path,
    })
}

fn apply_mutation(
    root: &mut Map<String, Value>,
    current: &Config,
    command: &ProfileCommand,
    config_path: &Path,
) -> Result<bool> {
    match command {
        ProfileCommand::Disable { profile } => {
            let resolved = find_profile(current, profile)?;
            set_profile_hidden(root, &resolved.name, true)
        }
        ProfileCommand::Enable { profile } => {
            let resolved = find_profile(current, profile)?;
            set_profile_hidden(root, &resolved.name, false)
        }
        ProfileCommand::Theme { profile, theme } => {
            let resolved = find_profile(current, profile)?;
            if let Some(theme) = theme {
                validate_theme_name(theme)?;
            }
            set_profile_theme(root, &resolved.name, theme.as_deref())
        }
        ProfileCommand::Default { profile } => {
            let resolved = find_profile(current, profile)?;
            let value = Value::String(resolved.name);
            let changed = root.get("default_profile") != Some(&value);
            root.insert("default_profile".to_owned(), value);
            Ok(changed)
        }
        ProfileCommand::Add {
            name,
            program,
            args,
            theme,
        } => {
            anyhow::ensure!(
                !current
                    .profiles
                    .iter()
                    .any(|profile| profile.name.eq_ignore_ascii_case(name)),
                "profile {name:?} already exists"
            );
            if let Some(theme) = theme {
                validate_theme_name(theme)?;
            }
            let profiles = profiles_array_mut(root)?;
            let mut value = Map::new();
            value.insert("name".to_owned(), json!(name));
            value.insert("program".to_owned(), json!(program));
            value.insert("args".to_owned(), json!(args));
            if let Some(theme) = theme {
                value.insert("theme".to_owned(), json!(theme));
            }
            profiles.push(Value::Object(value));
            Ok(true)
        }
        ProfileCommand::Remove { profile } => {
            let resolved = find_profile(current, profile)?;
            anyhow::ensure!(
                !Config::defaults(Some(config_path), None)
                    .profiles
                    .iter()
                    .any(|detected| detected.name.eq_ignore_ascii_case(&resolved.name)),
                "cannot remove detected profile {name:?}",
                name = resolved.name
            );
            anyhow::ensure!(
                current.profiles[current.default_profile].name != resolved.name,
                "cannot remove active default profile {name:?}",
                name = resolved.name
            );
            let profiles = root
                .get_mut("profiles")
                .and_then(Value::as_array_mut)
                .context("configuration profiles must be an array")?;
            let before = profiles.len();
            profiles.retain(|value| {
                value
                    .get("name")
                    .and_then(Value::as_str)
                    .is_none_or(|name| !name.eq_ignore_ascii_case(&resolved.name))
            });
            Ok(profiles.len() != before)
        }
        ProfileCommand::List | ProfileCommand::Themes => unreachable!(),
    }
}

fn find_profile(config: &Config, requested: &str) -> Result<Profile> {
    config
        .profiles
        .iter()
        .find(|profile| profile.name.eq_ignore_ascii_case(requested))
        .cloned()
        .with_context(|| format!("profile {requested:?} is not available"))
}

fn profiles_array_mut(root: &mut Map<String, Value>) -> Result<&mut Vec<Value>> {
    let profiles = root
        .entry("profiles".to_owned())
        .or_insert_with(|| Value::Array(Vec::new()));
    profiles
        .as_array_mut()
        .context("configuration profiles must be an array")
}

fn configured_profile_indices(root: &Map<String, Value>, name: &str) -> Vec<usize> {
    root.get("profiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, value)| {
            value
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|configured| configured.eq_ignore_ascii_case(name))
                .then_some(index)
        })
        .collect()
}

fn set_profile_hidden(root: &mut Map<String, Value>, name: &str, hidden: bool) -> Result<bool> {
    let indices = configured_profile_indices(root, name);
    if indices.is_empty() {
        if !hidden {
            return Ok(false);
        }
        let profiles = profiles_array_mut(root)?;
        let mut value = Map::new();
        value.insert("name".to_owned(), json!(name));
        value.insert("hidden".to_owned(), Value::Bool(true));
        profiles.push(Value::Object(value));
        return Ok(true);
    }

    let mut changed = false;
    let profiles = profiles_array_mut(root)?;
    for index in indices {
        let object = profiles[index]
            .as_object_mut()
            .context("each profile must be an object")?;
        if hidden {
            if object.get("hidden") != Some(&Value::Bool(true)) {
                object.insert("hidden".to_owned(), Value::Bool(true));
                changed = true;
            }
        } else if object.get("hidden") == Some(&Value::Bool(true)) {
            object.remove("hidden");
            changed = true;
        }
    }
    Ok(changed)
}

fn set_profile_theme(
    root: &mut Map<String, Value>,
    name: &str,
    theme: Option<&str>,
) -> Result<bool> {
    let indices = configured_profile_indices(root, name);
    if indices.is_empty() {
        let Some(theme) = theme else {
            return Ok(false);
        };
        let profiles = profiles_array_mut(root)?;
        let mut value = Map::new();
        value.insert("name".to_owned(), json!(name));
        value.insert("theme".to_owned(), json!(theme));
        profiles.push(Value::Object(value));
        return Ok(true);
    }

    let mut changed = false;
    let profiles = profiles_array_mut(root)?;
    for index in indices {
        let object = profiles[index]
            .as_object_mut()
            .context("each profile must be an object")?;
        match theme {
            Some(theme) => {
                if object.get("theme").and_then(Value::as_str) != Some(theme) {
                    object.insert("theme".to_owned(), json!(theme));
                    changed = true;
                }
            }
            None => {
                if object.remove("theme").is_some() {
                    changed = true;
                }
            }
        }
    }
    Ok(changed)
}

fn validate_theme_name(theme: &str) -> Result<()> {
    let themes = profile_theme_names()?;
    anyhow::ensure!(
        themes.iter().any(|available| available == theme),
        "unknown theme {theme:?}; available themes: {}",
        themes.join(", ")
    );
    Ok(())
}

pub(crate) fn profile_theme_names() -> Result<Vec<String>> {
    let assets = ZettaAssets;
    let mut names = BTreeSet::new();
    for path in assets.list("themes/")? {
        if !path.ends_with(".json") {
            continue;
        }
        let Some(bytes) = assets.load(&path)? else {
            continue;
        };
        let family: theme_settings::ThemeFamilyContent = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing bundled theme {path:?}"))?;
        names.extend(family.themes.into_iter().map(|theme| theme.name));
    }
    names.extend(user_theme_names(&themes_dir())?);
    Ok(names.into_iter().collect())
}

fn user_theme_names(themes_dir: &Path) -> Result<Vec<String>> {
    if !themes_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(themes_dir)
        .with_context(|| format!("reading theme directory {}", themes_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let bytes = fs::read(&path).with_context(|| format!("reading theme {}", path.display()))?;
        let Ok(family) = theme_settings::deserialize_user_theme(&bytes) else {
            continue;
        };
        names.extend(family.themes.into_iter().map(|theme| theme.name));
    }
    Ok(names.into_iter().collect())
}

fn read_config_source(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(source) => Ok(source),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok("{}".to_owned()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn save_config(path: &Path, source: &str) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, format!("{source}\n")).with_context(|| format!("writing {}", path.display()))
}

pub(crate) fn profile_operation_help(operation: Option<&str>) -> &'static str {
    match operation {
        Some("list") => {
            "List all resolved profiles\n\nUsage: zetta profile list [OPTIONS]\n\nPrints one resolved profile name per line, including hidden profiles.\n\nOptions:\n  -c, --config PATH  Use a configuration file\n  -h, --help         Print help"
        }
        Some("themes") => {
            "List available profile themes\n\nUsage: zetta profile themes [OPTIONS]\n\nPrints sorted, deduplicated bundled and installed theme names, one per line.\n\nOptions:\n  -c, --config PATH  Use a configuration file\n  -h, --help         Print help"
        }
        Some("disable") => {
            "Hide a profile\n\nUsage: zetta profile disable PROFILE [OPTIONS]\n\nDisabling an already hidden profile succeeds without changing the file.\n\nOptions:\n  -c, --config PATH  Use a configuration file\n  -h, --help         Print help"
        }
        Some("enable") => {
            "Show a profile\n\nUsage: zetta profile enable PROFILE [OPTIONS]\n\nEnabling an already visible profile succeeds without changing the file.\n\nOptions:\n  -c, --config PATH  Use a configuration file\n  -h, --help         Print help"
        }
        Some("theme") => {
            "Set or reset a profile theme\n\nUsage: zetta profile theme PROFILE THEME [OPTIONS]\n       zetta profile theme PROFILE --reset [OPTIONS]\n\nTHEME must be listed by `zetta profile themes`.\n\nOptions:\n  -r, --reset        Remove the profile theme override\n  -c, --config PATH  Use a configuration file\n  -h, --help         Print help"
        }
        Some("default") => {
            "Set the default profile\n\nUsage: zetta profile default PROFILE [OPTIONS]\n\nOptions:\n  -c, --config PATH  Use a configuration file\n  -h, --help         Print help"
        }
        Some("add") => {
            "Add a custom profile\n\nUsage: zetta profile add NAME --program PROGRAM [OPTIONS]\n\nOptions:\n  -p, --program PROGRAM  Program to launch\n  -a, --arg ARG          Add a repeatable program argument\n  -t, --theme THEME      Set a profile theme\n  -c, --config PATH      Use a configuration file\n  -h, --help             Print help"
        }
        Some("remove") => {
            "Remove a custom profile\n\nUsage: zetta profile remove PROFILE [OPTIONS]\n\nDetected profiles and the active default profile cannot be removed.\n\nOptions:\n  -c, --config PATH  Use a configuration file\n  -h, --help         Print help"
        }
        _ => {
            "Manage Zetta profiles\n\nUsage: zetta profile list [OPTIONS]\n       zetta profile themes [OPTIONS]\n       zetta profile disable PROFILE [OPTIONS]\n       zetta profile enable PROFILE [OPTIONS]\n       zetta profile theme PROFILE THEME [OPTIONS]\n       zetta profile theme PROFILE --reset [OPTIONS]\n       zetta profile default PROFILE [OPTIONS]\n       zetta profile add NAME --program PROGRAM [--arg ARG ...] [--theme THEME] [OPTIONS]\n       zetta profile remove PROFILE [OPTIONS]\n\nOperations:\n  list       List all resolved profiles, including hidden profiles\n  themes     List available bundled and installed themes\n  disable    Hide a profile\n  enable     Show a profile\n  theme      Set or reset a profile theme\n  default    Set the default profile\n  add        Add a custom profile\n  remove     Remove a custom profile\n\nThe -c/--config option may appear anywhere after `profile`. Mutations are validated before saving and request a best-effort live reload from a matching Zetta process.\n\nOptions:\n  -c, --config PATH  Use a configuration file\n  -h, --help         Print help"
        }
    }
}

pub(crate) fn reload_after_mutation(result: &ProfileCommandResult) {
    match request_existing_process_configuration_reload(&result.config_path) {
        Ok(true) => {}
        Ok(false) => eprintln!(
            "Notice: the profile change was saved, but no matching Zetta process was running; live state was not refreshed."
        ),
        Err(error) => eprintln!(
            "Notice: the profile change was saved, but live state was not refreshed ({error:#})."
        ),
    }
}

#[cfg(test)]
#[path = "tests/profile_cli.rs"]
mod tests;
