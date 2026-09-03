use super::*;
use std::collections::BTreeMap;

use crate::project_commands::{
    MAX_SHELL_COMMAND_BYTES, validate_command_environment_entry, validate_command_string,
};
use crate::run_command::{RunCommandRegistry, RunPaneIdentity, RunRegistration, RunWaitRequest};

/// Maximum UTF-8 payload accepted for one command-pane invocation. The process
/// control framing has a little extra room for JSON and authentication fields.
pub(crate) const MAX_PANE_COMMAND_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaneCommand {
    pub(crate) direction: Option<PaneDirection>,
    pub(crate) label: Option<String>,
    pub(crate) pane: Option<String>,
    pub(crate) overlay: Option<PaneOverlayRequest>,
    pub(crate) stack: bool,
    pub(crate) list: bool,
    pub(crate) command: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShellCommandRequest {
    pub(crate) command: String,
    pub(crate) arguments: Vec<String>,
    pub(crate) environment: BTreeMap<String, String>,
}

struct ResolvedPaneOverlay {
    text: Option<String>,
    font_size: Option<OverlayFontSize>,
    opacity: Option<f32>,
    color: Option<Hsla>,
}

fn resolve_pane_overlay(
    request: Option<PaneOverlayRequest>,
) -> Result<Option<ResolvedPaneOverlay>> {
    request
        .map(|request| {
            let color = request
                .color
                .map(|value| {
                    overlay_color_from_value(&value)
                        .with_context(|| format!("invalid overlay color {value:?}"))
                })
                .transpose()?;
            Ok(ResolvedPaneOverlay {
                text: request.text,
                font_size: request.font_size,
                opacity: request.opacity.map(|percent| f32::from(percent) / 100.),
                color,
            })
        })
        .transpose()
}

fn apply_pane_overlay(pane: &mut TerminalPane, overlay: Option<ResolvedPaneOverlay>) {
    if let Some(overlay) = overlay {
        pane.overlay_text = overlay.text;
        pane.overlay_font_size = overlay.font_size;
        pane.overlay_opacity = overlay.opacity;
        pane.overlay_color = overlay.color;
    }
}

pub(crate) fn parse_pane_direction(value: &str) -> Option<PaneDirection> {
    match value {
        "left" => Some(PaneDirection::Left),
        "right" => Some(PaneDirection::Right),
        "up" => Some(PaneDirection::Up),
        "down" => Some(PaneDirection::Down),
        _ => None,
    }
}

pub(crate) fn pane_direction_split(direction: PaneDirection) -> (SplitAxis, SplitPosition) {
    match direction {
        PaneDirection::Left => (SplitAxis::Vertical, SplitPosition::Before),
        PaneDirection::Right => (SplitAxis::Vertical, SplitPosition::After),
        PaneDirection::Up => (SplitAxis::Horizontal, SplitPosition::Before),
        PaneDirection::Down => (SplitAxis::Horizontal, SplitPosition::After),
    }
}

pub(crate) fn pane_command_byte_len(command: &[String]) -> usize {
    command.iter().map(String::len).sum::<usize>() + command.len().saturating_sub(1)
}

pub(crate) fn quote_pane_command_for_shell(profile: &Shell, command: &[String]) -> Result<String> {
    anyhow::ensure!(!command.is_empty(), "pane command must not be empty");
    anyhow::ensure!(
        pane_command_byte_len(command) <= MAX_PANE_COMMAND_BYTES,
        "pane command exceeds the {} KiB limit",
        MAX_PANE_COMMAND_BYTES / 1024
    );

    let kind = if is_wsl_shell(profile) {
        task::ShellKind::Posix
    } else {
        #[cfg(windows)]
        if msys2_profile(profile).is_some() {
            task::ShellKind::Posix
        } else {
            ShellBuilder::new(profile, true).kind()
        }
        #[cfg(not(windows))]
        {
            ShellBuilder::new(profile, false).kind()
        }
    };

    let program = kind
        .try_quote_prefix_aware(&command[0])
        .map(|value| value.into_owned())
        .context("pane command contains a value that cannot be quoted")?;
    let mut quoted = String::with_capacity(pane_command_byte_len(command));
    quoted.push_str(&program);
    for argument in &command[1..] {
        quoted.push(' ');
        let argument = kind
            .try_quote(argument)
            .map(|value| value.into_owned())
            .context("pane command contains a value that cannot be quoted")?;
        quoted.push_str(&argument);
    }
    Ok(quoted)
}

fn shell_kind_for_profile(profile: &Shell) -> task::ShellKind {
    if is_wsl_shell(profile) {
        task::ShellKind::Posix
    } else {
        #[cfg(windows)]
        if msys2_profile(profile).is_some() {
            task::ShellKind::Posix
        } else {
            ShellBuilder::new(profile, true).kind()
        }
        #[cfg(not(windows))]
        {
            ShellBuilder::new(profile, false).kind()
        }
    }
}

fn quote_shell_value(kind: task::ShellKind, value: &str) -> Result<String> {
    kind.try_quote(value)
        .map(|value| value.into_owned())
        .context("shell command contains a value that cannot be quoted")
}

fn quote_powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn append_shell_arguments(
    kind: task::ShellKind,
    command: &str,
    arguments: &[String],
) -> Result<String> {
    let mut command = command.to_owned();
    for argument in arguments {
        anyhow::ensure!(
            !argument.contains('\0'),
            "shell command arguments must not contain NUL"
        );
        command.push(' ');
        command.push_str(&quote_shell_value(kind, argument)?);
    }
    Ok(command)
}

/// Builds the command text injected into an already-running profile shell.
///
/// The configured command remains raw so shell expansion and operators retain
/// their normal meaning. Only invocation arguments and environment values are
/// quoted. Environment changes are enclosed in a shell-specific scope.
pub(crate) fn shell_command_for_profile(
    profile: &Shell,
    request: &ShellCommandRequest,
) -> Result<String> {
    validate_command_string(&request.command)?;
    let kind = shell_kind_for_profile(profile);
    let command = append_shell_arguments(kind, &request.command, &request.arguments)?;
    for (name, value) in &request.environment {
        validate_command_environment_entry(name, value, "shell command environment")?;
    }
    let payload_bytes = request.command.len()
        + request.arguments.iter().map(String::len).sum::<usize>()
        + request
            .environment
            .iter()
            .map(|(name, value)| name.len() + value.len())
            .sum::<usize>();
    anyhow::ensure!(
        payload_bytes <= MAX_SHELL_COMMAND_BYTES,
        "shell command request exceeds the {} KiB limit",
        MAX_SHELL_COMMAND_BYTES / 1024
    );
    if request.environment.is_empty() {
        anyhow::ensure!(
            command.len() <= MAX_SHELL_COMMAND_BYTES,
            "shell command exceeds the {} KiB limit",
            MAX_SHELL_COMMAND_BYTES / 1024
        );
        return Ok(command);
    }

    let scoped = match kind {
        task::ShellKind::Posix => {
            let assignments = request
                .environment
                .iter()
                .map(|(name, value)| Ok(format!("{name}={}", quote_shell_value(kind, value)?)))
                .collect::<Result<Vec<_>>>()?
                .join(" ");
            format!("( export {assignments}; {command} )")
        }
        task::ShellKind::Csh | task::ShellKind::Tcsh => {
            let assignments = request
                .environment
                .iter()
                .map(|(name, value)| {
                    Ok(format!("setenv {name} {}", quote_shell_value(kind, value)?))
                })
                .collect::<Result<Vec<_>>>()?
                .join("; ");
            format!("( {assignments}; {command} )")
        }
        task::ShellKind::Fish => {
            let assignments = request
                .environment
                .iter()
                .map(|(name, value)| {
                    Ok(format!(
                        "set -lx {name} {}",
                        quote_shell_value(kind, value)?
                    ))
                })
                .collect::<Result<Vec<_>>>()?
                .join("; ");
            format!("begin; {assignments}; {command}; end")
        }
        task::ShellKind::PowerShell | task::ShellKind::Pwsh => {
            let mut setup = vec![
                "$zetta_old_environment = @{}".to_owned(),
                "$zetta_had_environment = @{}".to_owned(),
            ];
            let mut assignments = Vec::with_capacity(request.environment.len());
            let mut restores = Vec::with_capacity(request.environment.len());
            for (name, value) in &request.environment {
                let path = quote_powershell_literal(&format!("Env:{name}"));
                let variable = quote_powershell_literal(name);
                let value = quote_shell_value(kind, value)?;
                setup.push(format!(
                    "$zetta_had_environment[{path}] = Test-Path -LiteralPath {path}"
                ));
                setup.push(format!(
                    "$zetta_old_environment[{path}] = [System.Environment]::GetEnvironmentVariable({variable}, 'Process')"
                ));
                assignments.push(format!("Set-Item -LiteralPath {path} -Value {value}"));
                restores.push(format!(
                    "if ($zetta_had_environment[{path}]) {{ Set-Item -LiteralPath {path} -Value $zetta_old_environment[{path}] }} else {{ Remove-Item -LiteralPath {path} -ErrorAction SilentlyContinue }}"
                ));
            }
            format!(
                "& {{ {}; try {{ {}; {} }} finally {{ {} }} }}",
                setup.join("; "),
                assignments.join("; "),
                command,
                restores.join("; ")
            )
        }
        task::ShellKind::Nushell => {
            let assignments = request
                .environment
                .iter()
                .map(|(name, value)| {
                    Ok(format!("$env.{name} = {}", quote_shell_value(kind, value)?))
                })
                .collect::<Result<Vec<_>>>()?
                .join("; ");
            format!("do {{ {assignments}; {command} }}")
        }
        task::ShellKind::Cmd => {
            let assignments = request
                .environment
                .iter()
                .map(|(name, value)| Ok(format!("set {name}={}", quote_shell_value(kind, value)?)))
                .collect::<Result<Vec<_>>>()?
                .join(" && ");
            // A nested cmd process gives the interactive cmd.exe a scoped
            // environment, while preserving cmd's raw command syntax.
            format!("cmd.exe /D /S /C \"{assignments} && {command}\"")
        }
        task::ShellKind::Xonsh => {
            let assignments = request
                .environment
                .iter()
                .map(|(name, value)| {
                    Ok(format!(
                        "{prefix}{name} = {value}",
                        prefix = "$",
                        name = name,
                        value = quote_shell_value(kind, value)?
                    ))
                })
                .collect::<Result<Vec<_>>>()?
                .join("; ");
            format!("( {assignments}; {command} )")
        }
        task::ShellKind::Rc => {
            let assignments = request
                .environment
                .iter()
                .map(|(name, value)| Ok(format!("{name}={}", quote_shell_value(kind, value)?)))
                .collect::<Result<Vec<_>>>()?
                .join("; ");
            format!("( {assignments}; {command} )")
        }
        task::ShellKind::Elvish => {
            let entries = request
                .environment
                .iter()
                .map(|(name, value)| {
                    Ok(format!(
                        "{} {}",
                        quote_shell_value(kind, name)?,
                        quote_shell_value(kind, value)?
                    ))
                })
                .collect::<Result<Vec<_>>>()?
                .join(" ");
            format!("with-env [{entries}] {{ {command} }}")
        }
    };
    anyhow::ensure!(
        scoped.len() <= MAX_SHELL_COMMAND_BYTES,
        "shell command exceeds the {} KiB limit",
        MAX_SHELL_COMMAND_BYTES / 1024
    );
    Ok(scoped)
}

pub(crate) fn exact_pane_command_shell(
    profile: &Shell,
    command: &[String],
    wsl_directory: Option<&str>,
) -> Result<Shell> {
    anyhow::ensure!(!command.is_empty(), "pane command must not be empty");
    anyhow::ensure!(
        pane_command_byte_len(command) <= MAX_PANE_COMMAND_BYTES,
        "pane command exceeds the {} KiB limit",
        MAX_PANE_COMMAND_BYTES / 1024
    );

    if is_wsl_shell(profile) {
        let shell = wsl_shell_with_tracking(profile.clone(), wsl_directory, None);
        let Shell::WithArguments {
            program,
            mut args,
            title_override,
        } = shell
        else {
            anyhow::bail!("WSL profile did not produce a launcher command")
        };
        args.push("--exec".to_owned());
        args.extend(command.iter().cloned());
        return Ok(Shell::WithArguments {
            program,
            args,
            title_override,
        });
    }

    #[cfg(windows)]
    if let Some((root, shell)) = msys2_profile(profile) {
        let shell_name = match shell {
            Msys2Shell::Bash => "bash.exe",
            Msys2Shell::Zsh => "zsh.exe",
        };
        let shell = Shell::Program(
            root.join("usr")
                .join("bin")
                .join(shell_name)
                .display()
                .to_string(),
        );
        let command = quote_pane_command_for_shell(profile, command)?;
        let (program, args) =
            ShellBuilder::new(&shell, true).build(Some(format!("exec {command}")), &[]);
        return Ok(Shell::WithArguments {
            program,
            args,
            title_override: None,
        });
    }

    #[cfg(windows)]
    if cygwin_profile(profile).is_some() {
        let command = quote_pane_command_for_shell(profile, command)?;
        let (program, args) =
            ShellBuilder::new(profile, true).build(Some(format!("exec {command}")), &[]);
        return Ok(Shell::WithArguments {
            program,
            args,
            title_override: None,
        });
    }

    Ok(Shell::WithArguments {
        program: command[0].clone(),
        args: command[1..].to_vec(),
        title_override: None,
    })
}

fn available_pane_labels(tab: &Tab) -> String {
    tab.panes
        .iter()
        .map(TerminalPane::label)
        .collect::<Vec<_>>()
        .join(", ")
}

fn resolve_pane_id(tab: &Tab, requested: Option<&str>) -> Result<u64> {
    let Some(requested) = requested else {
        return Ok(tab.active_pane);
    };
    let matches = tab
        .panes
        .iter()
        .filter(|pane| pane.label() == requested)
        .map(|pane| pane.id)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [pane_id] => Ok(*pane_id),
        [] => anyhow::bail!(
            "no pane named {requested:?}; available panes: {}",
            available_pane_labels(tab)
        ),
        _ => anyhow::bail!(
            "pane label {requested:?} is ambiguous; available panes: {}",
            available_pane_labels(tab)
        ),
    }
}

impl Zetta {
    pub(crate) fn register_run_wait(
        &self,
        request: RunWaitRequest,
        registry: &RunCommandRegistry,
        cx: &App,
    ) -> Result<RunRegistration> {
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.attention_id == request.owner.attention_id)
            .or_else(|| {
                self.background_sessions
                    .iter()
                    .find(|tab| tab.attention_id == request.owner.attention_id)
            })
            .context("the originating Zetta tab is no longer available")?;
        anyhow::ensure!(
            tab.panes.iter().any(|pane| {
                pane.routing_id == request.owner.routing_id
                    || pane
                        .stack
                        .entries
                        .iter()
                        .any(|entry| entry.routing_id == request.owner.routing_id)
            }),
            "the originating pane is no longer available"
        );

        let labels = tab
            .panes
            .iter()
            .map(TerminalPane::label)
            .collect::<Vec<_>>();
        let mut dependencies = Vec::with_capacity(request.dependencies.len());
        for label in &request.dependencies {
            let matches = tab
                .panes
                .iter()
                .filter(|pane| pane.label() == *label)
                .map(|pane| RunPaneIdentity::new(tab.attention_id, pane.routing_id))
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [dependency] => dependencies.push(*dependency),
                [] => anyhow::bail!(
                    "no base pane named {label:?} in the originating tab; available panes: {}",
                    labels.join(", ")
                ),
                _ => anyhow::bail!("base pane label {label:?} is ambiguous in the originating tab"),
            }
        }

        // The dependency's PTY can already have a foreground command while
        // its OSC command-start marker is still queued for the terminal entity
        // update. Observe the OS foreground process at this registration point
        // so a stale successful result cannot release a second wait early.
        for dependency in &dependencies {
            let Some(terminal) = tab
                .panes
                .iter()
                .find(|pane| pane.routing_id == dependency.routing_id)
                .and_then(|pane| pane.terminal.as_ref())
            else {
                continue;
            };
            if terminal.read(cx).foreground_process_is_shell_context_now() == Some(false) {
                registry.command_started(*dependency, "foreground command".to_owned());
            }
        }

        registry.register(
            request.owner,
            dependencies,
            request.allow_failure,
            request.command,
        )
    }

    pub(crate) fn command_pane_labels_for_attention(
        &self,
        attention_id: Option<u64>,
    ) -> Vec<String> {
        let tab = match attention_id {
            Some(attention_id) => self
                .tabs
                .iter()
                .find(|tab| tab.attention_id == attention_id)
                .or_else(|| {
                    self.background_sessions
                        .iter()
                        .find(|tab| tab.attention_id == attention_id)
                }),
            None => self.tabs.get(self.active_tab),
        };
        tab.map(|tab| tab.panes.iter().map(TerminalPane::label).collect())
            .unwrap_or_default()
    }

    pub(crate) fn run_command_pane(
        &mut self,
        request: PaneCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        anyhow::ensure!(
            !request.list,
            "pane list requests are handled before dispatch"
        );
        anyhow::ensure!(
            request.direction.is_some() || request.overlay.is_none(),
            "pane overlays require --direction"
        );
        if request.direction.is_some() {
            self.run_split_command_pane(request, window, cx)
        } else if request.stack {
            self.run_stacked_command_pane(request, window, cx)
        } else {
            self.run_direct_command_pane(request, window, cx)
        }
    }

    fn run_split_command_pane(
        &mut self,
        request: PaneCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let direction = request
            .direction
            .context("pane split direction is required")?;
        let label = request.label;
        let overlay = resolve_pane_overlay(request.overlay)?;
        let (axis, position) = pane_direction_split(direction);
        let tab_index = self.active_tab;
        let inherit_working_directory = self
            .effective_config()
            .working_directory_scope
            .inherits_for_new_pane();
        let working_directory_configured = self.effective_config().working_directory_configured;
        let project = self.active_project_config().cloned();
        let (
            tab_id,
            active_pane_id,
            tab_theme_override,
            profile,
            inherited_working_directory,
            inherited_wsl_directory,
        ) = {
            let tab = self.tabs.get(tab_index).context("there is no active tab")?;
            anyhow::ensure!(
                can_add_panes(tab.panes.len(), 1),
                "the active tab has reached the {MAX_PANES_PER_TAB}-pane limit"
            );
            if let Some(label) = label.as_deref() {
                anyhow::ensure!(
                    !tab.panes.iter().any(|pane| pane.label() == label),
                    "pane label {label:?} is already in use"
                );
            }
            let active_pane = tab
                .active_pane()
                .context("the active tab has no active pane")?;
            (
                tab.id,
                tab.active_pane,
                tab.theme_override.clone(),
                active_pane.profile.clone(),
                inherit_working_directory
                    .then(|| {
                        (!is_wsl_shell(&active_pane.profile.command))
                            .then(|| active_pane.working_directory(cx))
                            .flatten()
                    })
                    .flatten(),
                inherit_working_directory
                    .then(|| active_pane.wsl_working_directory(cx))
                    .flatten(),
            )
        };
        let (working_directory, wsl_directory) = launch_working_directory(
            &profile,
            inherited_working_directory,
            inherited_wsl_directory,
            self.working_directory.clone(),
            working_directory_configured,
        );
        let shell =
            exact_pane_command_shell(&profile.command, &request.command, wsl_directory.as_deref())?;
        let terminal_theme = resolve_terminal_theme(
            None,
            tab_theme_override.as_deref(),
            &profile,
            project.as_deref(),
            cx,
        )
        .context("could not resolve the active profile theme")?;
        let mut settings = TerminalSpawnSettings::current(cx);
        let path_hyperlink_regexes = settings.path_hyperlink_regexes(true);
        let pane_id = self.next_pane_id;
        let wsl_cwd_file = wsl_cwd_tracking_file(&profile, pane_id);
        let terminals_resized_by_split = matches!(axis, SplitAxis::Vertical)
            .then(|| {
                self.tabs[tab_index]
                    .panes
                    .iter()
                    .flat_map(TerminalPane::all_terminals)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for terminal in terminals_resized_by_split {
            terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
        }

        self.next_pane_id += 1;
        self.projects.inherit_pane_root(active_pane_id, pane_id);
        self.pane_controls_hidden_for
            .extend(default_hidden_pane_controls(
                self.launch_config.pane_controls_hidden_by_default,
                [pane_id],
            ));
        let tab = &mut self.tabs[tab_index];
        anyhow::ensure!(
            tab.layout.split(active_pane_id, axis, pane_id, position),
            "could not split the active pane"
        );
        tab.maximized_pane = None;
        let mut pane =
            TerminalPane::new(pane_id, profile.clone()).with_wsl_cwd_file(wsl_cwd_file.clone());
        if let Some(label) = label {
            pane = pane.with_generated_label(label);
        }
        apply_pane_overlay(&mut pane, overlay);
        tab.push_pane(pane);
        tab.activate_pane(pane_id);
        self.spawn_terminal(
            TerminalSpawnRequest {
                shell: Some(shell),
                working_directory,
                wsl_directory,
                wsl_cwd_file,
                terminal_theme,
                path_hyperlink_regexes,
                ..TerminalSpawnRequest::new(tab_id, pane_id, profile)
            },
            &settings,
            window,
            cx,
        );
        self.focus_active(window, cx);
        self.sync_visible_terminals(cx);
        cx.notify();
        Ok(())
    }

    fn run_direct_command_pane(
        &mut self,
        request: PaneCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let (pane_id, view, profile) = {
            let tab = self
                .tabs
                .get(self.active_tab)
                .context("there is no active tab")?;
            let pane_id = resolve_pane_id(tab, request.pane.as_deref())?;
            let pane = tab.pane(pane_id).context("target pane no longer exists")?;
            anyhow::ensure!(
                !pane.base_exited,
                "pane {label:?} has no running base shell",
                label = pane.label()
            );
            (pane_id, pane.view.clone(), pane.profile.command.clone())
        };
        let command = quote_pane_command_for_shell(&profile, &request.command)?;
        let tab = self
            .tabs
            .get_mut(self.active_tab)
            .context("there is no active tab")?;
        tab.activate_stack_entry(pane_id, PaneStackSelection::Base);
        if let Some(pane) = tab.pane_mut(pane_id)
            && view.is_none()
        {
            pane.pending_command = Some(command.clone());
        }
        if let Some(view) = view {
            view.update(cx, |view, cx| {
                view.apply_input(&TerminalInput::Text(format!("{command}\r")), cx)
            });
        }
        self.focus_active(window, cx);
        self.sync_visible_terminals(cx);
        cx.notify();
        Ok(())
    }

    pub(crate) fn run_shell_command(
        &mut self,
        request: ShellCommandRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let (pane_id, view, profile) = {
            let tab = self
                .tabs
                .get(self.active_tab)
                .context("there is no active tab")?;
            let pane = tab
                .active_pane()
                .context("the active tab has no active pane")?;
            anyhow::ensure!(
                !pane.base_exited,
                "the active pane has no running base shell"
            );
            (pane.id, pane.view.clone(), pane.profile.command.clone())
        };
        let command = shell_command_for_profile(&profile, &request)?;
        let tab = self
            .tabs
            .get_mut(self.active_tab)
            .context("there is no active tab")?;
        tab.activate_stack_entry(pane_id, PaneStackSelection::Base);
        if let Some(pane) = tab.pane_mut(pane_id)
            && view.is_none()
        {
            pane.pending_command = Some(command.clone());
        }
        if let Some(view) = view {
            view.update(cx, |view, cx| {
                view.apply_input(&TerminalInput::Text(format!("{command}\r")), cx)
            });
        }
        self.focus_active(window, cx);
        self.sync_visible_terminals(cx);
        cx.notify();
        Ok(())
    }

    fn run_stacked_command_pane(
        &mut self,
        request: PaneCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let project = self.active_project_config().cloned();
        let working_directory_configured = self.effective_config().working_directory_configured;
        let (
            tab_id,
            pane_id,
            tab_theme_override,
            profile,
            inherited_working_directory,
            inherited_wsl_directory,
        ) = {
            let tab = self
                .tabs
                .get(self.active_tab)
                .context("there is no active tab")?;
            let pane_id = resolve_pane_id(tab, request.pane.as_deref())?;
            let host = tab.pane(pane_id).context("target pane no longer exists")?;
            anyhow::ensure!(
                host.stack.entries.len() < MAX_PANES_PER_TAB.saturating_sub(1),
                "pane {label:?} has reached the {MAX_PANES_PER_TAB}-entry stacked-command limit",
                label = host.label()
            );
            (
                tab.id,
                pane_id,
                tab.theme_override.clone(),
                host.profile.clone(),
                host.working_directory(cx),
                host.wsl_working_directory(cx),
            )
        };
        let (working_directory, wsl_directory) = launch_working_directory(
            &profile,
            (!is_wsl_shell(&profile.command))
                .then_some(inherited_working_directory)
                .flatten(),
            is_wsl_shell(&profile.command)
                .then_some(inherited_wsl_directory)
                .flatten(),
            self.working_directory.clone(),
            working_directory_configured,
        );
        let terminal_theme = resolve_terminal_theme(
            None,
            tab_theme_override.as_deref(),
            &profile,
            project.as_deref(),
            cx,
        )
        .context("could not resolve the target profile theme")?;
        let command = quote_pane_command_for_shell(&profile.command, &request.command)?;
        let entry_id = self.next_pane_id;
        self.next_pane_id += 1;
        let mut settings = TerminalSpawnSettings::current(cx);
        let entry = StackedPane::new(
            entry_id,
            command.clone(),
            profile.clone(),
            working_directory.clone(),
            wsl_directory.clone(),
        );
        let inserted = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.pane_mut(pane_id))
            .is_some_and(|pane| pane.stack.push(entry));
        anyhow::ensure!(inserted, "could not add the stacked command");
        self.spawn_stacked_terminal(
            StackedTerminalSpawnRequest {
                tab_id,
                pane_id,
                entry_id,
                command,
                profile,
                working_directory,
                wsl_directory,
                terminal_theme,
            },
            &mut settings,
            true,
            window,
            cx,
        );
        self.focus_active(window, cx);
        self.sync_visible_terminals(cx);
        cx.notify();
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/command_panes.rs"]
mod tests;
