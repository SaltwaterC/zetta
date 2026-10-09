//! Approval of project commands changed outside Zetta's project editor.
//!
//! Runtime loads check the commands section's fingerprint before exposing commands.
//! Editor saves approve their submitted command snapshot as part of the user's Save
//! action. Other project settings follow normal configuration rules.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use crate::config::Config;
use crate::project::{ProjectConfig, ProjectRegistry};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectApprovalRecord {
    pub(crate) project: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) fingerprint: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectApproval {
    pub(crate) fingerprint: String,
    /// Prepared off the render path, and JSON-escaped so control characters in
    /// repository content cannot impersonate the surrounding prompt text.
    pub(crate) review: String,
}

/// Only the registered-command section participates in approval. Environment
/// overrides inside a command are part of that command; other project settings
/// follow the normal configuration rules without a separate approval step.
pub(crate) fn command_approval(source: &str) -> Result<ProjectApproval> {
    let object: Map<String, Value> = serde_json::from_str(source)?;
    let commands = object
        .get("commands")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let settings = canonical_value(serde_json::json!({"commands": commands}));
    let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&settings)?));
    Ok(ProjectApproval {
        fingerprint,
        review: serde_json::to_string_pretty(&settings)?,
    })
}

/// Saves the editor's configuration and approves commands it changed.
/// Saving unrelated fields must not approve unchanged commands loaded from disk.
/// Reading the file afterwards still goes through the approval gate:
/// an external replacement during the save cannot inherit this approval.
pub(crate) fn save_from_editor(
    root: &Path,
    base: &Config,
    source: &str,
    registry_path: PathBuf,
    original_commands_fingerprint: &str,
) -> Result<(PathBuf, ProjectRegistry, ProjectConfig)> {
    let approval = command_approval(source)?;
    let mut registry = ProjectRegistry::load_from(registry_path)?;
    let path = crate::project_form::save(root, base, source)?;
    if approval.fingerprint != original_commands_fingerprint {
        registry.approve(root, &approval.fingerprint)?;
        registry.save()?;
    }
    let config = ProjectConfig::load_in_registry(root, base, &registry)?;
    Ok((path, registry, config))
}

/// serde_json's map can preserve insertion order through dependency feature
/// unification, so sorting must be explicit at every depth.
fn canonical_value(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<_> = object.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonical_value(value)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_value).collect()),
        value => value,
    }
}

impl ProjectConfig {
    pub(crate) fn parse_registered(
        source: &str,
        root: &Path,
        base: &Config,
        registry: &ProjectRegistry,
    ) -> Result<Self> {
        let mut project = Self::parse(source, root, base)?;
        if project.commands.is_empty() {
            return Ok(project);
        }
        let approval = command_approval(source)?;
        if !registry.is_approved(&project.root, &approval.fingerprint) {
            project.commands.clear();
            project.pending_approval = Some(approval);
        }
        Ok(project)
    }
}

#[cfg(test)]
#[path = "tests/project_trust.rs"]
mod tests;
