//! Profile overlays read by the process which will start the pane.
//! Partial entries preserve a discovered command and can override its backend;
//! the top-level `windows_console_backend` is the backend of every profile that
//! does not name one.
use crate::{ConsoleBackend, ProfileCommand, ProfileDefinition};
use anyhow::{Context as _, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
struct ProfilesDocument {
    #[serde(
        default,
        rename = "windows_console_backend",
        deserialize_with = "backend_override"
    )]
    backend: Option<ConsoleBackend>,
    #[serde(default)]
    profiles: Vec<ProfileEntry>,
}
#[derive(Deserialize)]
struct ProfileEntry {
    name: String,
    #[serde(default)]
    program: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(
        default,
        rename = "windows_console_backend",
        deserialize_with = "backend_override"
    )]
    backend: Option<ConsoleBackend>,
}
fn backend_override<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ConsoleBackend>, D::Error> {
    ConsoleBackend::deserialize(deserializer).map(Some)
}
pub(crate) struct ConfiguredProfile {
    name: String,
    command: Option<ProfileCommand>,
    backend: Option<ConsoleBackend>,
}
/// The profile entries of a configuration file, and the backend every profile
/// without one of its own runs on.
#[derive(Default)]
pub(crate) struct Configured {
    pub(crate) backend: ConsoleBackend,
    pub(crate) profiles: Vec<ConfiguredProfile>,
}
pub(crate) fn configured_profiles(config_path: &Path) -> Result<Configured> {
    if !config_path.is_file() {
        return Ok(Configured::default());
    }
    let contents = std::fs::read_to_string(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let document: ProfilesDocument = serde_json::from_str(&contents)
        .with_context(|| format!("parsing the profiles in {}", config_path.display()))?;
    Ok(Configured {
        backend: document.backend.unwrap_or_default(),
        profiles: document
            .profiles
            .into_iter()
            .map(|entry| ConfiguredProfile {
                name: entry.name,
                command: entry
                    .program
                    .map(|program| ProfileCommand::with_args(program, entry.args)),
                backend: entry.backend,
            })
            .collect(),
    })
}
/// Overlays `configured` on the discovered `profiles`, which have no backend
/// of their own until the file's default reaches them here.
pub(crate) fn merge(profiles: &mut Vec<ProfileDefinition>, configured: Configured) {
    for profile in profiles.iter_mut() {
        profile.command.console_backend = configured.backend;
    }
    for profile in configured.profiles {
        if let Some(existing) = profiles
            .iter_mut()
            .find(|existing| existing.name.eq_ignore_ascii_case(&profile.name))
        {
            let backend = profile.backend.unwrap_or(existing.command.console_backend);
            if let Some(command) = profile.command {
                existing.command = command;
            }
            existing.command.console_backend = backend;
        } else if let Some(mut command) = profile.command {
            command.console_backend = profile.backend.unwrap_or(configured.backend);
            profiles.push(ProfileDefinition {
                name: profile.name,
                command,
            });
        }
    }
}

#[cfg(test)]
#[path = "tests/config_file.rs"]
mod tests;
