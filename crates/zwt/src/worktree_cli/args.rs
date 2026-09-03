//! Parsing a `zetta wt` / `zwt` command line.
//!
//! The same commands are reachable under both names, so every parser takes the
//! [`WorktreeInvocation`] it was reached by and reports errors in that name.

use super::*;

pub fn parse_worktree_args(arguments: &[OsString]) -> Result<WorktreeCommand> {
    parse_worktree_args_for(arguments, WorktreeInvocation::Standalone)
}

pub fn parse_worktree_args_for(
    arguments: &[OsString],
    invocation: WorktreeInvocation,
) -> Result<WorktreeCommand> {
    if arguments.is_empty() {
        anyhow::bail!(
            "{} requires an operation; run {} --help for usage",
            invocation.command(),
            invocation.command()
        );
    }
    let operation = arguments.first().map(|argument| argument.to_string_lossy());
    if operation.as_deref() == Some("--help") || operation.as_deref() == Some("-h") {
        println!("{}", worktree_help_for(invocation));
        std::process::exit(0);
    }

    let operation = operation.expect("worktree operation was checked above");
    match operation.as_ref() {
        "new" => parse_new_args(&arguments[1..], invocation),
        "done" => parse_done_args(&arguments[1..], invocation),
        "abort" => parse_abort_args(&arguments[1..], invocation),
        "status" => parse_no_arguments(
            "status",
            &arguments[1..],
            WorktreeCommand::Status,
            invocation,
        ),
        "sync" => parse_sync_args(&arguments[1..], invocation),
        "config" => parse_no_arguments(
            "config",
            &arguments[1..],
            WorktreeCommand::Config,
            invocation,
        ),
        unknown => {
            anyhow::bail!(
                "unknown {} operation {unknown:?}; run {} --help for usage",
                invocation.command(),
                invocation.command()
            )
        }
    }
}

pub(super) fn parse_new_args(
    arguments: &[OsString],
    invocation: WorktreeInvocation,
) -> Result<WorktreeCommand> {
    let mut path_only = false;
    let mut name = None;
    let mut copy_paths = Vec::new();
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--help" | "-h" => {
                println!("{}", worktree_new_help_for(invocation));
                std::process::exit(0);
            }
            PATH_ONLY_OPTION | "-P" => {
                anyhow::ensure!(!path_only, "{PATH_ONLY_OPTION} may only be specified once");
                path_only = true;
            }
            "--copy" | "-c" => {
                let path = arguments
                    .next()
                    .context("--copy requires a relative PATH")?;
                anyhow::ensure!(
                    !path.to_string_lossy().starts_with('-'),
                    "--copy requires a relative PATH"
                );
                copy_paths.push(validate_copy_path(Path::new(path))?);
            }
            value if value.starts_with('-') => {
                anyhow::bail!("unknown {} new option {value:?}", invocation.command())
            }
            value => {
                anyhow::ensure!(
                    name.is_none(),
                    "{} new accepts exactly one NAME",
                    invocation.command()
                );
                name = Some(value.to_owned());
            }
        }
    }
    let name = name.with_context(|| {
        format!(
            "{} new requires NAME; run {} new --help for usage",
            invocation.command(),
            invocation.command()
        )
    })?;
    anyhow::ensure!(
        !name.is_empty(),
        "{} new requires a non-empty NAME",
        invocation.command()
    );
    let copy_paths = validate_copy_paths(&copy_paths)?;
    Ok(WorktreeCommand::New {
        name,
        path_only,
        copy_paths,
    })
}

pub(super) fn parse_done_args(
    arguments: &[OsString],
    invocation: WorktreeInvocation,
) -> Result<WorktreeCommand> {
    let path_only = parse_path_only_args(arguments, "done", invocation, worktree_done_help_for)?;
    Ok(WorktreeCommand::Done { path_only })
}

pub(super) fn parse_abort_args(
    arguments: &[OsString],
    invocation: WorktreeInvocation,
) -> Result<WorktreeCommand> {
    let path_only = parse_path_only_args(arguments, "abort", invocation, worktree_abort_help_for)?;
    Ok(WorktreeCommand::Abort { path_only })
}

pub(super) fn parse_sync_args(
    arguments: &[OsString],
    invocation: WorktreeInvocation,
) -> Result<WorktreeCommand> {
    let mut commit = None;
    for argument in arguments {
        match argument.to_string_lossy().as_ref() {
            "--help" | "-h" => {
                println!("{}", worktree_sync_help_for(invocation));
                std::process::exit(0);
            }
            value if value.starts_with('-') => {
                anyhow::bail!("unknown {} sync argument {value:?}", invocation.command())
            }
            value => {
                anyhow::ensure!(
                    commit.is_none(),
                    "{} sync accepts at most one COMMIT",
                    invocation.command()
                );
                commit = Some(value.to_owned());
            }
        }
    }
    Ok(WorktreeCommand::Sync { commit })
}

pub(super) fn parse_path_only_args(
    arguments: &[OsString],
    operation: &str,
    invocation: WorktreeInvocation,
    help: fn(WorktreeInvocation) -> String,
) -> Result<bool> {
    let mut path_only = false;
    for argument in arguments {
        match argument.to_string_lossy().as_ref() {
            "--help" | "-h" => {
                println!("{}", help(invocation));
                std::process::exit(0);
            }
            PATH_ONLY_OPTION | "-P" => {
                anyhow::ensure!(!path_only, "{PATH_ONLY_OPTION} may only be specified once");
                path_only = true;
            }
            value => anyhow::bail!(
                "unknown {} {operation} argument {value:?}",
                invocation.command()
            ),
        }
    }
    Ok(path_only)
}

pub(super) fn parse_no_arguments(
    operation: &str,
    arguments: &[OsString],
    command: WorktreeCommand,
    invocation: WorktreeInvocation,
) -> Result<WorktreeCommand> {
    if arguments
        .iter()
        .any(|argument| matches!(argument.to_string_lossy().as_ref(), "--help" | "-h"))
    {
        println!(
            "{}",
            match operation {
                "status" => worktree_status_help_for(invocation),
                "config" => worktree_config_help_for(invocation),
                _ => worktree_help_for(invocation),
            }
        );
        std::process::exit(0);
    }
    anyhow::ensure!(
        arguments.is_empty(),
        "{} {operation} does not accept arguments; run {} {operation} --help for usage",
        invocation.command(),
        invocation.command()
    );
    Ok(command)
}

#[cfg(test)]
#[path = "../tests/worktree_cli/args.rs"]
mod tests;
