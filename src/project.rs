use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tempfile::NamedTempFile;

use crate::config::{Config, platform_config_dir};
use crate::project_commands::{
    RegisteredProjectCommand, parse_project_commands, validate_environment_entry,
};
use crate::worktree_detection::{WorktreeMetadata, detect_worktree_metadata};

pub(crate) const PROJECT_CONFIG_DIRECTORY: &str = ".zetta";
pub(crate) const PROJECT_CONFIG_FILE: &str = "config.json";
const PROJECT_REGISTRY_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub(crate) struct ProjectConfig {
    pub(crate) root: PathBuf,
    pub(crate) effective: Config,
    /// The fields explicitly present in the project file. `effective` also
    /// contains inherited values, so it cannot be used to determine project
    /// precedence when selecting a mode-specific theme.
    pub(crate) theme: Option<String>,
    pub(crate) dark_theme: Option<String>,
    pub(crate) environment: HashMap<String, String>,
    pub(crate) commands: std::collections::BTreeMap<String, RegisteredProjectCommand>,
    pub(crate) initial_split: Option<String>,
}

impl ProjectConfig {
    pub(crate) fn path_for(root: &Path) -> PathBuf {
        root.join(PROJECT_CONFIG_DIRECTORY)
            .join(PROJECT_CONFIG_FILE)
    }

    pub(crate) fn load(root: &Path, base: &Config) -> Result<Self> {
        let path = Self::path_for(root);
        let source = fs::read_to_string(&path)
            .with_context(|| format!("reading project configuration {}", path.display()))?;
        Self::parse(&source, root, base)
    }

    pub(crate) fn parse(source: &str, root: &Path, base: &Config) -> Result<Self> {
        let root = canonical_project_root(root)?;
        let path = Self::path_for(&root);
        let value: Value = serde_json::from_str(source)
            .with_context(|| format!("parsing project configuration {}", path.display()))?;
        let object = value
            .as_object()
            .context("project configuration root must be an object")?;
        validate_project_fields(object)?;

        let environment = object
            .get("env")
            .map(parse_project_environment)
            .transpose()?
            .unwrap_or_default();
        let commands = object
            .get("commands")
            .map(parse_project_commands)
            .transpose()?
            .unwrap_or_default();
        let initial_split = object
            .get("initial_split")
            .map(|value| {
                let name = value.as_str().context("initial_split must be a string")?;
                anyhow::ensure!(!name.trim().is_empty(), "initial_split must not be empty");
                Ok(name.to_owned())
            })
            .transpose()?;

        let mut overlay = object.clone();
        overlay.remove("env");
        overlay.remove("commands");
        overlay.remove("initial_split");
        let overlay_source = serde_json::to_string(&Value::Object(overlay))?;
        let mut effective = Config::parse_overlay(&overlay_source, base.clone(), &path)?;

        let theme = object
            .get("theme")
            .map(|value| {
                value
                    .as_str()
                    .context("theme must be a string")
                    .map(str::to_owned)
            })
            .transpose()?;
        let dark_theme = object
            .get("dark_theme")
            .map(|value| {
                value
                    .as_str()
                    .context("dark_theme must be a string")
                    .map(str::to_owned)
            })
            .transpose()?;

        effective.working_directory = Some(match object.get("working_directory") {
            Some(value) => resolve_project_working_directory(&root, value)?,
            None => root.clone(),
        });
        effective.working_directory_configured = true;

        if let Some(name) = initial_split.as_deref() {
            anyhow::ensure!(
                effective.pane_split_templates.contains_key(name),
                "initial_split {name:?} is not an available pane split template"
            );
        }

        Ok(Self {
            root,
            effective,
            theme,
            dark_theme,
            environment,
            commands,
            initial_split,
        })
    }
}

pub(crate) fn validate_project_fields(object: &Map<String, Value>) -> Result<()> {
    const FIELDS: &[&str] = &[
        "theme",
        "dark_theme",
        "working_directory",
        "default_profile",
        "profiles",
        "default_tab_icon",
        "env",
        "commands",
        "inactive_pane_opacity",
        "initial_split",
        "pane_split_templates",
    ];
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        anyhow::bail!("unrecognized project configuration field {field:?}");
    }
    Ok(())
}

fn parse_project_environment(value: &Value) -> Result<HashMap<String, String>> {
    let object = value
        .as_object()
        .context("env must be an object of strings")?;
    object
        .iter()
        .map(|(name, value)| {
            let value = value
                .as_str()
                .context("project environment values must be strings")?;
            validate_environment_entry(name, value, "project environment")?;
            Ok((name.clone(), value.to_owned()))
        })
        .collect()
}

fn resolve_project_working_directory(root: &Path, value: &Value) -> Result<PathBuf> {
    let relative = value
        .as_str()
        .context("working_directory must be a project-relative string")?;
    anyhow::ensure!(
        !relative.trim().is_empty(),
        "working_directory must not be empty"
    );
    let relative = Path::new(relative);
    anyhow::ensure!(
        !relative.is_absolute()
            && relative.components().all(|component| !matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )),
        "working_directory must stay inside the project"
    );
    let directory = fs::canonicalize(root.join(relative)).with_context(|| {
        format!(
            "canonicalizing project working directory {}",
            root.join(relative).display()
        )
    })?;
    anyhow::ensure!(
        directory.is_dir() && path_is_within(&directory, root),
        "working_directory must be an existing directory inside the project"
    );
    Ok(directory)
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectRegistry {
    path: PathBuf,
    roots: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectRootResolution {
    /// The registered project root used for registry identity and project
    /// opening. A managed worktree keeps its main repository here.
    pub(crate) root: Option<PathBuf>,
    /// The root whose `.zetta/config.json` supplies the effective project
    /// settings. A managed worktree with its own project file uses that file;
    /// otherwise it falls back to the registered main repository.
    pub(crate) config_root: Option<PathBuf>,
    /// Present only when `root` is a registered project reached through a
    /// Zetta-managed `wt/*` linked worktree. An unregistered or ordinary Git
    /// worktree deliberately falls through to normal directory matching.
    pub(crate) managed_worktree: Option<WorktreeMetadata>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectRegistryFile {
    version: u32,
    projects: Vec<PathBuf>,
}

impl ProjectRegistry {
    pub(crate) fn empty() -> Self {
        Self {
            path: platform_config_dir().join("projects.json"),
            roots: Vec::new(),
        }
    }

    pub(crate) fn load() -> Result<Self> {
        Self::load_from(platform_config_dir().join("projects.json"))
    }

    pub(crate) fn load_from(path: PathBuf) -> Result<Self> {
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self {
                    path,
                    roots: Vec::new(),
                });
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading project registry {}", path.display()));
            }
        };
        let file: ProjectRegistryFile = serde_json::from_str(&source)
            .with_context(|| format!("parsing project registry {}", path.display()))?;
        anyhow::ensure!(
            file.version == PROJECT_REGISTRY_VERSION,
            "unsupported project registry version {}; expected {PROJECT_REGISTRY_VERSION}",
            file.version
        );
        let mut roots: Vec<PathBuf> = Vec::with_capacity(file.projects.len());
        for root in file.projects {
            anyhow::ensure!(
                root.is_absolute(),
                "registered project roots must be absolute"
            );
            if !roots.iter().any(|candidate| paths_equal(candidate, &root)) {
                roots.push(root);
            }
        }
        roots.sort_by_key(|root| path_identity(root));
        Ok(Self { path, roots })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub(crate) fn contains(&self, root: &Path) -> bool {
        self.roots
            .iter()
            .any(|candidate| paths_equal(candidate, root))
    }

    pub(crate) fn matching_root(&self, directory: &Path) -> Option<&PathBuf> {
        self.roots
            .iter()
            .filter(|root| path_is_within(directory, root))
            .max_by_key(|root| root.components().count())
    }

    pub(crate) fn add(&mut self, root: &Path) -> Result<bool> {
        let root = canonical_project_root(root)?;
        if self.contains(&root) {
            return Ok(false);
        }
        self.roots.push(root);
        self.roots.sort_by_key(|root| path_identity(root));
        Ok(true)
    }

    pub(crate) fn remove(&mut self, root_or_child: &Path) -> Option<PathBuf> {
        let index = self
            .roots
            .iter()
            .enumerate()
            .filter(|(_, root)| path_is_within(root_or_child, root))
            .max_by_key(|(_, root)| root.components().count())
            .map(|(index, _)| index)?;
        Some(self.roots.remove(index))
    }

    pub(crate) fn save(&self) -> Result<()> {
        let parent = self
            .path
            .parent()
            .context("project registry path has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("creating project registry directory {}", parent.display()))?;
        let file = ProjectRegistryFile {
            version: PROJECT_REGISTRY_VERSION,
            projects: self.roots.clone(),
        };
        write_json_atomically(&self.path, &file)
    }
}

/// Resolve a directory to a registered project without invoking Git or
/// changing the registry.
///
/// A Zetta-managed `wt/*` linked worktree lives beside its main repository, so
/// a lexical ancestor lookup cannot find the main project. Git's linked
/// worktree metadata gives us that main root; it is trusted only when the main
/// root is already registered. The main root remains the registry identity,
/// while a worktree-local `.zetta/config.json`, when present, supplies the
/// effective settings. Everything else uses the existing directory based
/// lookup, preserving ordinary and detached worktree behavior.
pub(crate) fn resolve_registered_project(
    directory: &Path,
    registry: &ProjectRegistry,
) -> ProjectRootResolution {
    let worktree = detect_worktree_metadata(directory).ok().flatten();
    if let Some(worktree) = worktree
        && let Some(root) = registry.matching_root(&worktree.main_root).cloned()
    {
        let config_root = if ProjectConfig::path_for(&worktree.root).is_file() {
            worktree.root.clone()
        } else {
            root.clone()
        };
        return ProjectRootResolution {
            root: Some(root),
            config_root: Some(config_root),
            managed_worktree: Some(worktree),
        };
    }

    let root = registry.matching_root(directory).cloned();
    ProjectRootResolution {
        config_root: root.clone(),
        root,
        managed_worktree: None,
    }
}

pub(crate) fn resolve_registered_project_root(
    directory: &Path,
    registry: &ProjectRegistry,
) -> Option<PathBuf> {
    resolve_registered_project(directory, registry).root
}

pub(crate) fn resolve_registered_project_config_root(
    directory: &Path,
    registry: &ProjectRegistry,
) -> Option<PathBuf> {
    resolve_registered_project(directory, registry).config_root
}

pub(crate) fn is_registered_project_config_root(root: &Path, registry: &ProjectRegistry) -> bool {
    resolve_registered_project_config_root(root, registry)
        .is_some_and(|resolved| paths_equal(&resolved, root))
}

pub(crate) fn ensure_project_config(root: &Path) -> Result<PathBuf> {
    let root = canonical_project_root(root)?;
    let directory = root.join(PROJECT_CONFIG_DIRECTORY);
    fs::create_dir_all(&directory).with_context(|| {
        format!(
            "creating project configuration directory {}",
            directory.display()
        )
    })?;
    let path = directory.join(PROJECT_CONFIG_FILE);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(b"{}\n")
                .with_context(|| format!("writing project configuration {}", path.display()))?;
            file.sync_all()
                .with_context(|| format!("syncing project configuration {}", path.display()))?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("creating project configuration {}", path.display()));
        }
    }
    Ok(path)
}

/// The short label for a project root: its directory name, falling back to
/// `Project` for a root that has none, such as a filesystem root.
pub(crate) fn project_display_name(root: &Path) -> &str {
    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Project")
}

pub(crate) fn canonical_project_root(root: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("canonicalizing project root {}", root.display()))?;
    anyhow::ensure!(
        root.is_dir(),
        "project root {} is not a directory",
        root.display()
    );
    Ok(root)
}

pub(crate) fn find_repository_root(directory: &Path) -> Result<Option<PathBuf>> {
    let directory = canonical_project_root(directory)?;
    for ancestor in directory.ancestors() {
        let marker = ancestor.join(".git");
        match fs::symlink_metadata(&marker) {
            Ok(_) => return Ok(Some(ancestor.to_path_buf())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading Git marker {}", marker.display()));
            }
        }
    }
    Ok(None)
}

pub(crate) fn discover_project_config(directory: &Path) -> Result<Option<PathBuf>> {
    let Some(root) = find_repository_root(directory)? else {
        return Ok(None);
    };
    Ok(ProjectConfig::path_for(&root).is_file().then_some(root))
}

pub(crate) fn is_wsl_unc_path(path: &Path) -> bool {
    let value = path.to_string_lossy();
    value.starts_with(r"\\wsl$\") || value.starts_with(r"\\wsl.localhost\")
}

pub(crate) fn path_is_within(path: &Path, root: &Path) -> bool {
    if cfg!(windows) {
        let path = path_identity(path);
        let root = path_identity(root);
        path == root
            || path
                .strip_prefix(&root)
                .is_some_and(|suffix| suffix.starts_with(['/', '\\']))
    } else {
        // Registry roots are canonical, so a query path that did not come
        // from the app (a CLI argument, or a pane whose working directory was
        // resolved through a symlink like macOS `/var` -> `/private/var`)
        // needs canonicalizing before a lexical prefix match means anything.
        // Paths that cannot be canonicalized (deleted directories, WSL UNC
        // paths) stay lexical.
        path.starts_with(root) || fs::canonicalize(path).is_ok_and(|path| path.starts_with(root))
    }
}

pub(crate) fn paths_equal(left: &Path, right: &Path) -> bool {
    if !cfg!(windows) {
        return left == right;
    }
    let left = left.to_string_lossy();
    let right = right.to_string_lossy();
    windows_paths_equal(&left, &right)
}

/// The Windows half of [`paths_equal`], split out so it can be tested from any
/// platform — the `cfg!(windows)` above is a runtime branch, so these rules are
/// otherwise never exercised off Windows.
///
/// Compares the normalised character streams instead of building normalised
/// `String`s: `config_for_pane` calls this once per registered project, several
/// times per frame, and [`path_identity`] allocates two or three strings a call.
fn windows_paths_equal(left: &str, right: &str) -> bool {
    normalized_windows_chars(left).eq(normalized_windows_chars(right))
}

/// A Windows path as the characters that decide its identity: the verbatim
/// prefix removed, separators unified, trailing separators dropped, lowercased.
fn normalized_windows_chars(value: &str) -> impl Iterator<Item = char> + '_ {
    // `fs::canonicalize` returns verbatim `\\?\`-prefixed paths, while
    // tempdir-style paths and CLI arguments do not; both spell the same
    // directory.
    let (prefix, rest) = match value.strip_prefix(r"\\?\UNC\") {
        Some(rest) => (r"\\", rest),
        None => match value.strip_prefix(r"\\?\") {
            Some(rest) => ("", rest),
            None => ("", value),
        },
    };
    prefix
        .chars()
        .chain(rest.trim_end_matches(['/', '\\']).chars())
        .map(|character| if character == '\\' { '/' } else { character })
        .flat_map(char::to_lowercase)
}

fn path_identity(path: &Path) -> String {
    let value = path.to_string_lossy();
    // `fs::canonicalize` returns verbatim `\\?\`-prefixed paths on Windows,
    // while tempdir-style paths and CLI arguments do not; both spell the same
    // directory, so normalize the prefix away before comparing.
    let value = if cfg!(windows) {
        match value.strip_prefix(r"\\?\UNC\") {
            Some(rest) => format!(r"\\{rest}"),
            // Not `map_or_else`: the fallback moves `value`, which the
            // `strip_prefix` borrow is still holding while the call is made.
            None => match value.strip_prefix(r"\\?\") {
                Some(rest) => rest.to_owned(),
                None => value.into_owned(),
            },
        }
    } else {
        value.into_owned()
    };
    let value = value.replace('\\', "/");
    let value = value.trim_end_matches('/');
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value.to_owned()
    }
}

pub(crate) fn write_text_atomically(path: &Path, text: &str) -> Result<()> {
    let parent = path.parent().context("file path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let mut temporary = NamedTempFile::new_in(parent)
        .with_context(|| format!("creating temporary file in {}", parent.display()))?;
    temporary
        .write_all(text.as_bytes())
        .with_context(|| format!("writing temporary file for {}", path.display()))?;
    if !text.ends_with('\n') {
        temporary.write_all(b"\n")?;
    }
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

fn write_json_atomically(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut source = serde_json::to_string_pretty(value)?;
    source.push('\n');
    write_text_atomically(path, &source)
}

#[cfg(test)]
#[path = "tests/project.rs"]
mod tests;
