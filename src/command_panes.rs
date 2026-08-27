use super::*;

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
    pub(crate) fn command_pane_labels(&self) -> Vec<String> {
        self.tabs
            .get(self.active_tab)
            .map(|tab| tab.panes.iter().map(TerminalPane::label).collect())
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
        let (tab_id, active_pane_id, profile, inherited_working_directory, inherited_wsl_directory) = {
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
        let terminal_theme = resolve_project_profile_theme(&profile, project.as_deref(), cx)
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
        self.spawn_terminal_with_shell(
            tab_id,
            pane_id,
            profile,
            shell,
            working_directory,
            wsl_directory,
            wsl_cwd_file,
            terminal_theme,
            &settings,
            path_hyperlink_regexes,
            false,
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

    fn run_stacked_command_pane(
        &mut self,
        request: PaneCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let project = self.active_project_config().cloned();
        let working_directory_configured = self.effective_config().working_directory_configured;
        let (tab_id, pane_id, profile, inherited_working_directory, inherited_wsl_directory) = {
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
        let terminal_theme = resolve_project_profile_theme(&profile, project.as_deref(), cx)
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
            tab_id,
            pane_id,
            entry_id,
            command,
            profile,
            working_directory,
            wsl_directory,
            terminal_theme,
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
