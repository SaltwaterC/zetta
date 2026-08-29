use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};

use crate::startup::format_help_table;
use crate::{
    config::Config,
    project::{
        ProjectConfig, ProjectRegistry, ProjectRootResolution, canonical_project_root,
        ensure_project_config, find_repository_root, is_wsl_unc_path, resolve_registered_project,
        resolve_registered_project_root,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProjectCommand {
    Add { path: Option<PathBuf> },
    List,
    Remove { path: Option<PathBuf> },
    Open { path: Option<PathBuf> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectOpenTarget {
    pub(crate) root: PathBuf,
    /// A managed worktree keeps the directory from which `project open` was
    /// requested, while an ordinary project uses its configured working
    /// directory and therefore leaves this unset.
    pub(crate) working_directory: Option<PathBuf>,
}

pub(crate) fn parse_project_args(arguments: &[OsString]) -> Result<ProjectCommand> {
    if arguments
        .iter()
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        let operation = arguments
            .iter()
            .find(|argument| !argument.to_string_lossy().starts_with('-'))
            .map(|argument| argument.to_string_lossy());
        println!("{}", project_help(operation.as_deref()));
        std::process::exit(0);
    }

    let operation = arguments
        .first()
        .context("zetta project requires an operation; run `zetta project --help` for usage")?
        .to_string_lossy();
    let arguments = &arguments[1..];
    match operation.as_ref() {
        "add" => parse_path_command(arguments).map(|path| ProjectCommand::Add { path }),
        "list" => {
            anyhow::ensure!(
                arguments.is_empty(),
                "project list does not accept arguments; run `zetta project list --help`"
            );
            Ok(ProjectCommand::List)
        }
        "remove" => parse_path_command(arguments).map(|path| ProjectCommand::Remove { path }),
        "open" => parse_path_command(arguments).map(|path| ProjectCommand::Open { path }),
        unknown => anyhow::bail!(
            "unknown zetta project operation {unknown:?}; run `zetta project --help` for usage"
        ),
    }
}

fn parse_path_command(arguments: &[OsString]) -> Result<Option<PathBuf>> {
    let mut path = None;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--path" | "-p" => {
                anyhow::ensure!(path.is_none(), "--path may only be specified once");
                path = Some(
                    arguments
                        .next()
                        .context("--path requires a directory")?
                        .into(),
                );
            }
            value if value.starts_with('-') => {
                anyhow::bail!("unknown project option {value:?}")
            }
            _ => {
                anyhow::ensure!(path.is_none(), "only one project path may be specified");
                path = Some(argument.into());
            }
        }
    }
    Ok(path)
}

pub(crate) fn run_non_open(command: &ProjectCommand, base: &Config) -> Result<bool> {
    match command {
        ProjectCommand::Add { path } => {
            let requested = absolute_path(path.as_deref())?;
            let mut registry = ProjectRegistry::load()?;
            let resolution = resolve_registered_project(&requested, &registry);
            let root = if resolution.managed_worktree.is_some() {
                resolution
                    .root
                    .context("managed worktree has no registered main project")?
            } else if path.is_none() && !is_wsl_unc_path(&requested) {
                find_repository_root(&requested)?.unwrap_or(requested)
            } else {
                requested
            };
            ensure_project_config(&root)?;
            ProjectConfig::load(&root, base)?;
            let changed = registry.add(&root)?;
            if changed {
                registry.save()?;
            }
            println!("{}", root.display());
            Ok(changed)
        }
        ProjectCommand::List => {
            for root in ProjectRegistry::load()?.roots() {
                println!("{}", root.display());
            }
            Ok(false)
        }
        ProjectCommand::Remove { path } => {
            let requested = absolute_path_without_requiring_existence(path.as_deref())?;
            let mut registry = ProjectRegistry::load()?;
            let remove_from =
                resolve_registered_project_root(&requested, &registry).unwrap_or(requested.clone());
            let removed = registry.remove(&remove_from).with_context(|| {
                format!(
                    "{} is not inside a registered Zetta project",
                    requested.display()
                )
            })?;
            registry.save()?;
            println!("{}", removed.display());
            Ok(true)
        }
        ProjectCommand::Open { .. } => anyhow::bail!("project open must be handled by startup"),
    }
}

pub(crate) fn resolve_open_target(path: Option<&Path>) -> Result<ProjectOpenTarget> {
    let requested = absolute_path_without_requiring_existence(path)?;
    let registry = ProjectRegistry::load()?;
    resolve_open_target_in_registry(&requested, &registry)
}

fn resolve_open_target_in_registry(
    requested: &Path,
    registry: &ProjectRegistry,
) -> Result<ProjectOpenTarget> {
    let resolution = resolve_registered_project(requested, registry);
    let ProjectRootResolution {
        root,
        managed_worktree,
    } = resolution;
    let root = root.with_context(|| {
        format!(
            "{} is not inside a registered Zetta project",
            requested.display()
        )
    })?;
    let working_directory = managed_worktree
        .as_ref()
        .and_then(|_| fs::canonicalize(requested).ok())
        .filter(|directory| directory.is_dir());
    Ok(ProjectOpenTarget {
        root,
        working_directory,
    })
}

#[allow(dead_code)]
pub(crate) fn resolve_open_root(path: Option<&Path>) -> Result<PathBuf> {
    Ok(resolve_open_target(path)?.root)
}

#[allow(dead_code)]
pub(crate) fn current_registered_project() -> Result<Option<PathBuf>> {
    let current = absolute_path(None)?;
    Ok(resolve_registered_project(&current, &ProjectRegistry::load()?).root)
}

pub(crate) fn current_project_target() -> Result<Option<ProjectOpenTarget>> {
    let current = absolute_path(None)?;
    let registry = ProjectRegistry::load()?;
    let resolution = resolve_registered_project(&current, &registry);
    let ProjectRootResolution {
        root,
        managed_worktree,
    } = resolution;
    Ok(root.map(|root| ProjectOpenTarget {
        root,
        working_directory: managed_worktree.map(|_| current.clone()),
    }))
}

/// Loads the `.zetta/config.json` of the registered project containing the current
/// directory, overlaid on `base`.
///
/// Subcommands that render or report anything configurable need this and not
/// `load_startup_config` alone, or they answer for the wrong project. `zetta vi`
/// runs as its own process, so without it the editor highlighted with the
/// application theme inside a project that selects a different one.
pub(crate) fn current_project_config(base: &Config) -> Result<Option<ProjectConfig>> {
    current_project_target()?
        .as_ref()
        .map(|target| ProjectConfig::load(&target.root, base))
        .transpose()
}

fn absolute_path(path: Option<&Path>) -> Result<PathBuf> {
    canonical_project_root(&absolute_path_without_requiring_existence(path)?)
}

fn absolute_path_without_requiring_existence(path: Option<&Path>) -> Result<PathBuf> {
    let path = path
        .map(Path::to_path_buf)
        .unwrap_or(std::env::current_dir()?);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

pub(crate) fn project_help(operation: Option<&str>) -> String {
    match operation {
        Some("add") => {
            format!(
                "Register a Zetta project\n\nUsage: zetta project add [PATH]\n       zetta project add --path PATH\n\nCreates PATH/.zetta/config.json with an empty object when it does not exist, validates it, and records the canonical project root. Without PATH, the nearest native Git repository root is used, falling back to the current directory. WSL uses the exact current directory and is never scanned. Register only trusted projects: pane templates may launch commands.\n\nOptions:\n{}",
                format_help_table([
                    ("-p, --path PATH", "Project root"),
                    ("-h, --help", "Print help"),
                ])
            )
        }
        Some("list") => {
            format!(
                "List registered Zetta projects\n\nUsage: zetta project list\n\nPrints one canonical project root per line.\n\nOptions:\n{}",
                format_help_table([("-h, --help", "Print help")])
            )
        }
        Some("remove") => {
            format!(
                "Unregister a Zetta project\n\nUsage: zetta project remove [PATH]\n       zetta project remove --path PATH\n\nRemoves the containing project from Zetta's registry. The project's .zetta/config.json is never deleted.\n\nOptions:\n{}",
                format_help_table([
                    ("-p, --path PATH", "Project root or a path inside it"),
                    ("-h, --help", "Print help"),
                ])
            )
        }
        Some("open") => {
            format!(
                "Open a registered Zetta project\n\nUsage: zetta project open [PATH]\n       zetta project open --path PATH\n\nOpens the containing registered project in a new active tab of the running Zetta process, or starts Zetta when needed. A Zetta-managed wt/* worktree uses the registered main repository's configuration while preserving the requested worktree directory.\n\nOptions:\n{}",
                format_help_table([
                    ("-p, --path PATH", "Project root or a path inside it"),
                    ("-h, --help", "Print help"),
                ])
            )
        }
        _ => {
            let commands = format_help_table([
                (
                    "add",
                    "Create or validate project configuration and register the project",
                ),
                ("list", "List registered projects"),
                (
                    "remove",
                    "Unregister a project without deleting its configuration",
                ),
                ("open", "Open a registered project in a new active tab"),
            ]);
            format!(
                "Manage Zetta projects\n\nUsage: zetta project <COMMAND> [OPTIONS]\n\nCommands:\n{commands}\n\nRun `zetta project <COMMAND> --help` for command-specific help."
            )
        }
    }
}

#[cfg(test)]
#[path = "tests/project_cli.rs"]
mod tests;
