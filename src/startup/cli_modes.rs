//! The `zetta` subcommands that never open a window of their own.
//!
//! Each function here is one arm of [`super::run`]'s dispatch, and each is
//! reached only for its own [`StartupMode`] variant. Anything that has to
//! reach a *running* window goes over the control socket
//! (`process_control.rs`) and is answered by `process_control_loop.rs`
//! instead.

use super::*;

use super::arg_parsing::{AttentionCommand, TerminalResize};
use crate::profile_cli::ProfileCommand;

/// `zetta pane wait` — registers the wait with the owning pane, runs the
/// wrapped command once its dependencies report ready, then exits with the
/// command's own status.
pub(super) fn run_wait_command(command: PaneWaitCommand) -> Result<()> {
    let process_id = env::var("ZETTA_PROCESS_ID")
        .context("zetta pane wait must be invoked inside a Zetta terminal")?
        .parse::<u32>()
        .context("ZETTA_PROCESS_ID must be a positive process ID")?;
    let attention_id = env::var("ZETTA_ATTENTION_ID")
        .context("zetta pane wait must be invoked inside a Zetta terminal")?
        .parse::<u64>()
        .context("ZETTA_ATTENTION_ID must be a positive attention ID")?;
    // `ZETTA_PANE_ID` is retained as a compatibility fallback for shells
    // started before the stable routing variable was introduced. New panes
    // always use the routing ID, so pane moves continue to follow the stable
    // identity rather than the remapped layout ID.
    let routing_id = match env::var("ZETTA_PANE_ROUTING_ID") {
        Ok(value) => value
            .parse::<u64>()
            .context("ZETTA_PANE_ROUTING_ID must be a positive pane routing ID")?,
        Err(_) => env::var("ZETTA_PANE_ID")
            .context("ZETTA_PANE_ROUTING_ID and ZETTA_PANE_ID are missing; restart this pane to enable zetta pane wait")?
            .parse::<u64>()
            .context("ZETTA_PANE_ID must be a positive pane routing ID")?,
    };
    anyhow::ensure!(
        process_id != 0,
        "ZETTA_PROCESS_ID must be a positive process ID"
    );
    anyhow::ensure!(
        attention_id != 0,
        "ZETTA_ATTENTION_ID must be a positive attention ID"
    );
    anyhow::ensure!(routing_id != 0, "pane routing ID must be positive");

    let mut connection = request_process_run_wait(
        process_id,
        RunWaitRequest {
            owner: crate::run_command::RunPaneIdentity::new(attention_id, routing_id),
            dependencies: command.dependencies,
            allow_failure: command.allow_failure,
            command: command.command,
        },
    )?;
    let child_status = std::process::Command::new(&connection.command[0])
        .args(&connection.command[1..])
        .status()
        .with_context(|| format!("failed to start command {:?}", connection.command[0]))?;
    let exit_code = child_status.code();
    connection.complete(exit_code)?;
    std::process::exit(exit_code.unwrap_or(1));
}

/// A registered project command, run in the project the current directory
/// belongs to.
pub(super) fn run_registered_project_command(invocation: &ProjectCommandInvocation) -> Result<()> {
    let (base, _) = load_startup_config(None, None);
    let project = crate::project_cli::current_project_config(&base)?
        .context("the current directory is not inside a registered Zetta project")?;
    match invocation {
        ProjectCommandInvocation::List => {
            for name in project.commands.keys() {
                println!("{name}");
            }
        }
        ProjectCommandInvocation::Run { name, arguments } => {
            let command = project
                .commands
                .get(name)
                .with_context(|| format!("project command {name:?} is not registered"))?;
            let request = crate::command_panes::ShellCommandRequest {
                command: command.command.clone(),
                arguments: arguments.clone(),
                environment: merge_command_environment(&project.environment, &command.environment),
            };
            anyhow::ensure!(
                request_existing_process_shell_command(request)?,
                "no running Zetta process accepted the project command request"
            );
        }
    }
    Ok(())
}

/// The `zetta project` subcommands that only edit the registry. `open` is not
/// one of them: it opens a window, so [`super::run`] routes it to the
/// application path instead.
pub(super) fn run_project_registry_command(
    command: &crate::project_cli::ProjectCommand,
) -> Result<()> {
    let (base, _) = load_startup_config(None, None);
    if crate::project_cli::run_non_open(command, &base)? {
        request_existing_process_projects_reload().log_err();
    }
    Ok(())
}

/// `zetta edit` — hands the file to `$EDITOR` when one is configured, and to
/// the built-in viewer otherwise. Never returns.
pub(super) fn run_editor(arguments: &[String], delete_after: bool) -> Result<()> {
    let (arguments, cleanup_path) = if delete_after {
        let path = terminal_view::claim_scrollback_for_editor(Path::new(&arguments[0]))
            .context("claiming the managed scrollback file")?;
        (vec![path.to_string_lossy().into_owned()], Some(path))
    } else {
        (arguments.to_vec(), None)
    };
    let editor = env::var("EDITOR")
        .ok()
        .filter(|editor| !editor.trim().is_empty());
    let result: Result<i32> = (|| {
        if let Some(editor) = editor {
            let mut editor_parts = task::ShellKind::system()
                .split(&editor)
                .filter(|parts| !parts.is_empty())
                .context("EDITOR does not contain a command")?;
            let program = editor_parts.remove(0);
            std::process::Command::new(&program)
                .args(editor_parts)
                .args(paths_for_external_editor(&arguments))
                .status()
                .with_context(|| format!("failed to start editor {program:?}"))
                .map(|status| status.code().unwrap_or(1))
        } else {
            Ok(run_builtin_viewer(arguments))
        }
    })();
    if let Some(path) = cleanup_path {
        let _ = terminal_view::remove_scrollback_file(&path);
    }
    std::process::exit(result?);
}

/// `zetta vi` — the built-in viewer, which owns the process from here. Never
/// returns.
pub(super) fn run_vi(arguments: Vec<String>) -> Result<()> {
    std::process::exit(run_builtin_viewer(arguments));
}

fn run_builtin_viewer(arguments: Vec<String>) -> i32 {
    #[cfg(feature = "syntax-highlighting")]
    {
        vi_syntax::run(arguments)
    }
    #[cfg(not(feature = "syntax-highlighting"))]
    {
        busy_v::run(arguments)
    }
}

/// `zetta pane` — lists the running process's command panes, or asks it to
/// open one.
pub(super) fn run_pane_command(request: &crate::command_panes::PaneCommand) -> Result<()> {
    if request.list {
        let labels = request_existing_process_pane_labels()?
            .context("no running Zetta process accepted the pane list request")?;
        for label in labels {
            println!("{label}");
        }
        return Ok(());
    }
    anyhow::ensure!(
        request_existing_process_pane(request.clone())?,
        "no running Zetta process accepted the pane command request"
    );
    Ok(())
}

/// `zetta attention` — marks the tab this shell belongs to, and optionally
/// raises a desktop notification for it.
pub(super) fn run_attention_command(command: &AttentionCommand) -> Result<()> {
    let inherited_process_id =
        env::var("ZETTA_PROCESS_ID").context("zetta attention must run inside a Zetta terminal")?;
    let inherited_attention_id = env::var("ZETTA_ATTENTION_ID")
        .context("zetta attention must run inside a Zetta terminal")?;
    let (process_id, attention_id) =
        parse_attention_target(&inherited_process_id, &inherited_attention_id)?;
    let target = NotificationTarget {
        process_id,
        attention_id,
    };
    let accepted = request_process_tab_attention(
        target.process_id,
        TabAttentionRequest {
            attention_id: target.attention_id,
            summary: command.notification.summary.clone(),
            body: command.notification.body.clone(),
        },
    )?;
    anyhow::ensure!(accepted, "the originating Zetta tab is no longer available");
    #[cfg(feature = "notifications")]
    if command.notify {
        run_notification(&command.notification, Some(target))?;
    }
    #[cfg(not(feature = "notifications"))]
    if command.notify {
        anyhow::bail!("desktop notifications are disabled in this build");
    }
    Ok(())
}

/// `zetta profile` — edits the configured profiles, then tells any running
/// process to pick the change up.
pub(super) fn run_profile_command(
    command: ProfileCommand,
    config_path: Option<&Path>,
) -> Result<()> {
    let result = crate::profile_cli::run(command, config_path)?;
    if result.changed {
        crate::profile_cli::reload_after_mutation(&result);
    }
    Ok(())
}

/// `zetta size` — reports the invoking terminal's grid, or asks it to resize.
pub(super) fn run_terminal_size_command(json: bool, resize: Option<TerminalResize>) -> Result<()> {
    if let Some(resize) = resize {
        return request_terminal_resize(resize.columns, resize.rows);
    }
    print_terminal_size(json);
    Ok(())
}

/// `zetta shell-integration --configure`.
pub(super) fn configure_shell_integration() -> Result<()> {
    println!(
        "{}",
        shell_integration_configuration_message(&configure_current_shell_integration()?)
    );
    Ok(())
}

fn shell_integration_configuration_message(
    configuration: &ShellIntegrationConfiguration,
) -> String {
    match configuration {
        ShellIntegrationConfiguration::Written(path) => format!(
            "Added Zetta shell integration to {}. Start a new shell or reload this file to enable it.",
            path.display()
        ),
        ShellIntegrationConfiguration::AlreadyPresent(path) => format!(
            "Zetta shell integration is already present in {}; no changes made.",
            path.display()
        ),
    }
}

/// `zetta tabicon --list`.
pub(super) fn list_tab_icons() -> Result<()> {
    for icon in tab_icon_completion_names() {
        println!("{icon}");
    }
    Ok(())
}

/// `zetta tabicon NAME`.
pub(super) fn set_tab_icon(icon: Option<IconName>) -> Result<()> {
    anyhow::ensure!(
        request_existing_process_tab_icon(icon)?,
        "no running Zetta process accepted the tab icon request"
    );
    Ok(())
}

/// `zetta theme [pane] NAME`.
pub(super) fn set_theme(scope: ThemeScope, theme: Option<String>) -> Result<()> {
    anyhow::ensure!(
        request_existing_process_theme(scope, theme)?,
        "no running Zetta process accepted the theme request"
    );
    Ok(())
}

/// `zetta theme --list`, answered by the running process because the theme
/// registry includes whatever extensions it has loaded.
pub(super) fn list_themes() -> Result<()> {
    let themes = request_existing_process_theme_list()?
        .context("no running Zetta process accepted the theme list request")?;
    for theme in themes {
        println!("{theme}");
    }
    Ok(())
}

/// `zetta splits --list`, read from configuration rather than from a running
/// process, and from the current project's overlay when there is one.
pub(super) fn list_pane_splits() -> Result<()> {
    let (base, _) = load_startup_config(None, None);
    let project = crate::project_cli::current_project_config(&base)?;
    let config = project.as_ref().map_or(&base, |project| &project.effective);
    for name in configured_split_names(config) {
        println!("{name}");
    }
    Ok(())
}

/// `zetta overlay`.
pub(super) fn set_pane_overlay(request: PaneOverlayRequest) -> Result<()> {
    anyhow::ensure!(
        request_existing_process_pane_overlay(request)?,
        "no running Zetta process accepted the pane overlay request"
    );
    Ok(())
}

/// `zetta mux ...`, forwarded to the multiplexer client so the subcommand and
/// the `zmux` binary cannot accept different arguments.
pub(super) fn run_mux_command(arguments: &[OsString], config_path: Option<PathBuf>) -> Result<()> {
    // The same reader `src/bin/zmux.rs` uses, so `zetta mux` and `zmux`
    // resolve the identity identically.
    #[cfg(feature = "session-persistence")]
    if crate::mux_identity::command_uses_an_identity(arguments) {
        return zmux::run_with_defaults(
            arguments,
            zmux::ClientDefaults {
                identity_paths: crate::mux_identity::configured_identity_paths(config_path),
            },
        );
    }
    #[cfg(not(feature = "session-persistence"))]
    let _ = config_path;
    zmux::run(arguments)
}

/// `zetta --register-windows-shell`.
#[cfg(windows)]
pub(super) fn register_windows_shell(
    shortcut_path: &Path,
    config_path: Option<&Path>,
    keymap_path: Option<PathBuf>,
) -> Result<()> {
    let (config, _) = load_startup_config(config_path, keymap_path);
    windows_integration::register_shell_integration(
        shortcut_path,
        &config.profiles,
        &config.hidden_profiles,
    )
}

#[cfg(test)]
#[path = "../tests/startup/cli_modes.rs"]
mod tests;
