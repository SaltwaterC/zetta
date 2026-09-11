//! The profile entries of `config.json`, read for their commands alone.
//!
//! The application parses the whole file strictly — an unknown key is an error,
//! and so is a `null` where a value belongs — because a misspelled setting the
//! user cannot see is worse than a startup failure. This reader is the
//! opposite: it is used by a daemon that has to start a shell on behalf of
//! somebody else, so it takes the profiles it understands and ignores
//! everything around them. The strict reading still happens, in the window that
//! owns the file.

use std::path::Path;

use anyhow::{Context as _, Result};
use serde::Deserialize;

use crate::{ProfileCommand, ProfileDefinition};

#[derive(Deserialize)]
struct ProfilesDocument {
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
}

pub(crate) fn configured_profiles(config_path: &Path) -> Result<Vec<ProfileDefinition>> {
    if !config_path.is_file() {
        return Ok(Vec::new());
    }
    let contents = std::fs::read_to_string(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let document: ProfilesDocument = serde_json::from_str(&contents)
        .with_context(|| format!("parsing the profiles in {}", config_path.display()))?;
    Ok(document
        .profiles
        .into_iter()
        .filter_map(|entry| {
            // A profile with arguments but no program is rejected by the
            // application's own parser; here it simply has nothing to run.
            let program = entry.program?;
            Some(ProfileDefinition {
                name: entry.name,
                command: ProfileCommand::with_args(program, entry.args),
            })
        })
        .collect())
}

/// Overlays configured profiles onto the discovered ones, matching by name the
/// way the application does: an existing name has its command replaced, a new
/// one is appended in the order the file lists it.
pub(crate) fn merge(profiles: &mut Vec<ProfileDefinition>, configured: Vec<ProfileDefinition>) {
    for profile in configured {
        if let Some(existing) = profiles
            .iter_mut()
            .find(|existing| existing.name.eq_ignore_ascii_case(&profile.name))
        {
            existing.command = profile.command;
        } else {
            profiles.push(profile);
        }
    }
}
