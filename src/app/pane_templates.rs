//! Applying a pane template to a tab, and replacing a pane from the command
//! line.
//!
//! Both resolve a template's leaves against the panes a tab already has, so an
//! applied template reuses a pane whose profile and directory already match
//! rather than restarting its shell. `ResolvedPaneSplitLeaf` is that decision,
//! and `pane_split_leaf_requires_restart` is what it turns on.

use super::*;

fn resolve_cli_replacement_profile(
    profiles: &[Profile],
    requested_name: Option<&str>,
    requested_theme: Option<&str>,
    launch_theme_override: Option<&(String, String)>,
) -> Option<Option<Profile>> {
    match requested_name {
        Some(requested_name) if !requested_name.is_empty() => {
            let mut profile = profiles
                .iter()
                .find(|profile| profile.name.eq_ignore_ascii_case(requested_name))
                .cloned()?;
            apply_launch_theme_override(&mut profile, launch_theme_override);
            if let Some(theme) = requested_theme {
                if theme.is_empty() {
                    return None;
                }
                profile.theme = Some(theme.to_owned());
                profile.dark_theme = Some(theme.to_owned());
            }
            Some(Some(profile))
        }
        Some(_) => None,
        None if requested_theme.is_some() => None,
        None => Some(None),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ResolvedPaneSplitLeaf {
    label: Option<String>,
    profile: Profile,
    environment: HashMap<String, String>,
    overlay_text: Option<String>,
    overlay_font_size: Option<OverlayFontSize>,
    overlay_opacity: Option<f32>,
    overlay_color: Option<Hsla>,
    /// Stacked commands to seed in this pane, already quoted for the resolved
    /// profile's shell the way `zetta pane --stack` quotes them.
    stack: Vec<String>,
}

fn resolve_pane_split_leaves(
    template: &PaneSplitTemplateConfig,
    inherited_profile: &Profile,
    profile_override: Option<&Profile>,
) -> Result<Vec<ResolvedPaneSplitLeaf>> {
    let fallback_profile = profile_override.unwrap_or(inherited_profile);
    template
        .pane_specifications()
        .into_iter()
        .map(|pane: PaneSplitPane| {
            let mut profile = pane.profile.unwrap_or_else(|| fallback_profile.clone());
            if let Some(command) = pane.command {
                profile.command = command.shell();
            }
            if let Some(theme) = pane.theme {
                profile.theme = Some(theme);
            }
            if let Some(dark_theme) = pane.dark_theme {
                profile.dark_theme = Some(dark_theme);
            }
            let (overlay_text, overlay_font_size, overlay_opacity, overlay_color) = match pane
                .overlay
            {
                Some(overlay) => (
                    overlay.text,
                    overlay.size.map(|size| match size {
                        PaneSplitOverlaySize::Small => OverlayFontSize::Small,
                        PaneSplitOverlaySize::Base => OverlayFontSize::Base,
                        PaneSplitOverlaySize::Large => OverlayFontSize::Large,
                        PaneSplitOverlaySize::ExtraLarge => OverlayFontSize::ExtraLarge,
                        PaneSplitOverlaySize::ExtraExtraLarge => OverlayFontSize::ExtraExtraLarge,
                        PaneSplitOverlaySize::ExtraExtraExtraLarge => {
                            OverlayFontSize::ExtraExtraExtraLarge
                        }
                    }),
                    overlay.opacity.map(|opacity| f32::from(opacity) / 100.),
                    overlay
                        .color
                        .map(|color| {
                            overlay_color_from_value(&color).with_context(|| {
                                format!("using pane template overlay color {color:?}")
                            })
                        })
                        .transpose()?,
                ),
                None => (None, None, None, None),
            };
            // Quoting uses the leaf's resolved shell, which is also the shell
            // `stacked_task_shell` runs the entry through.
            let stack = pane
                .stack
                .iter()
                .map(|command| {
                    let argv = std::iter::once(command.program.clone())
                        .chain(command.args.iter().cloned())
                        .collect::<Vec<_>>();
                    quote_pane_command_for_shell(&profile.command, &argv)
                        .with_context(|| format!("using stacked command {:?}", command.program))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(ResolvedPaneSplitLeaf {
                label: pane.label,
                profile,
                environment: pane.env,
                overlay_text,
                overlay_font_size,
                overlay_opacity,
                overlay_color,
                stack,
            })
        })
        .collect()
}

fn apply_pane_split_overlay(pane: &mut TerminalPane, leaf: &ResolvedPaneSplitLeaf) {
    pane.overlay_text = leaf.overlay_text.clone();
    pane.overlay_font_size = leaf.overlay_font_size;
    pane.overlay_opacity = leaf.overlay_opacity;
    pane.overlay_color = leaf.overlay_color;
}

fn pane_split_leaf_requires_restart(pane: &TerminalPane, leaf: &ResolvedPaneSplitLeaf) -> bool {
    pane.profile != leaf.profile
        || pane.environment_overrides != leaf.environment
        || pane.base_exited
        || pane.error.is_some()
        // A retained pane keeps the stack it already has, so seeding a declared
        // stack on top of it would append duplicates every time the template is
        // applied. Rebuilding the pane makes its stack exactly what the template
        // describes.
        || !leaf.stack.is_empty()
}

impl Zetta {
    pub(crate) fn apply_pane_split_template(
        &mut self,
        action: &ApplyPaneSplitTemplate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_pane_split_template_with_profile(&action.name, None, window, cx);
    }

    pub(crate) fn replace_active_pane_from_cli(
        &mut self,
        request: ReplacePaneRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if request.split.is_none() && request.profile.is_none() {
            return false;
        }
        let Some(profile_override) = resolve_cli_replacement_profile(
            &self.profiles,
            request.profile.as_deref(),
            request.theme.as_deref(),
            self.launch_theme_override.as_ref(),
        ) else {
            return false;
        };

        if let Some(name) = request.split {
            self.apply_pane_split_template_with_profile(&name, profile_override, window, cx)
        } else {
            self.replace_active_pane_profile(
                profile_override.expect("a profile is required without a split template"),
                window,
                cx,
            )
        }
    }

    pub(crate) fn apply_pane_split_template_with_profile(
        &mut self,
        name: &str,
        profile_override: Option<Profile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let project = self.active_project_config().cloned();
        let effective = project
            .as_ref()
            .map_or(&self.launch_config, |project| &project.effective)
            .clone();
        let templates = effective.pane_split_templates.clone();
        let Some(template) = templates.get(name) else {
            self.configuration_error =
                Some(format!("Pane split template {:?} is not configured", name));
            cx.notify();
            return false;
        };
        let Some(new_pane_count) = template.pane_count().checked_sub(1) else {
            return false;
        };
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return false;
        };
        if !can_add_panes(tab.panes.len(), new_pane_count) {
            return false;
        }
        let tab_id = tab.id;
        let tab_theme_override = tab.theme_override.clone();
        let active_pane_theme_override = tab
            .active_pane()
            .and_then(|pane| pane.theme_override.clone());
        let active_pane_id = tab.active_pane;
        let active_pane = tab.active_pane();
        let Some(active_profile) = tab.active_profile().cloned() else {
            return false;
        };
        let mut leaves =
            match resolve_pane_split_leaves(template, &active_profile, profile_override.as_ref()) {
                Ok(leaves) => leaves,
                Err(error) => {
                    self.configuration_error = Some(format!(
                        "Could not resolve pane split template {:?}: {error:#}",
                        name
                    ));
                    cx.notify();
                    return false;
                }
            };
        if let Some(project) = &project {
            for leaf in &mut leaves {
                let template_environment = std::mem::take(&mut leaf.environment);
                leaf.environment = project.environment.clone();
                leaf.environment.extend(template_environment);
            }
        }
        let terminal_themes = match self.resolve_pane_template_themes(
            &leaves,
            active_pane_theme_override.as_deref(),
            tab_theme_override.as_deref(),
            project.as_deref(),
            cx,
        ) {
            Ok(themes) => themes,
            Err(error) => {
                let message = format!("{error:#}");
                self.configuration_error = Some(message);
                cx.notify();
                return false;
            }
        };
        let active_leaf = &leaves[0];
        let replacing_active =
            active_pane.is_none_or(|pane| pane_split_leaf_requires_restart(pane, active_leaf));
        let inherit_working_directory = effective.working_directory_scope.inherits_for_new_pane();
        let inherited_working_directory = active_pane
            .filter(|_| inherit_working_directory)
            .filter(|pane| !is_wsl_shell(&pane.profile.command))
            .and_then(|pane| pane.working_directory(cx));
        let inherited_wsl_directory = active_pane
            .filter(|_| inherit_working_directory)
            .and_then(|pane| pane.wsl_working_directory(cx));
        let working_directories = leaves
            .iter()
            .map(|leaf| {
                launch_working_directory(
                    &leaf.profile,
                    inherited_working_directory.clone(),
                    inherited_wsl_directory.clone(),
                    effective.working_directory.clone(),
                    effective.working_directory_configured,
                )
            })
            .collect::<Vec<_>>();
        let terminal_settings = TerminalSpawnSettings::current(cx);

        if !replacing_active
            && let Some(terminal) = active_pane.and_then(|pane| pane.terminal.clone())
        {
            terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
        }

        let new_pane_ids = (0..new_pane_count).map(|_| {
            let pane_id = self.next_pane_id;
            self.next_pane_id += 1;
            pane_id
        });
        let existing_active_wsl_cwd_file = active_pane.and_then(|pane| pane.wsl_cwd_file.clone());
        let new_panes = new_pane_ids
            .enumerate()
            .map(|(index, pane_id)| {
                (
                    pane_id,
                    wsl_cwd_tracking_file(&leaves[index + 1].profile, pane_id),
                )
            })
            .collect::<Vec<_>>();
        for (pane_id, _) in &new_panes {
            self.projects.inherit_pane_root(active_pane_id, *pane_id);
        }
        let active_wsl_cwd_file = if replacing_active {
            wsl_cwd_tracking_file(&active_leaf.profile, active_pane_id)
        } else {
            existing_active_wsl_cwd_file
        };
        self.pane_controls_hidden_for
            .extend(default_hidden_pane_controls(
                self.launch_config.pane_controls_hidden_by_default,
                new_panes.iter().map(|(pane_id, _)| *pane_id),
            ));
        let mut all_pane_ids =
            std::iter::once(active_pane_id).chain(new_panes.iter().map(|(pane_id, _)| *pane_id));
        let replacement = pane_layout_from_configured_template(&templates, name, &mut all_pane_ids)
            .expect("the configured pane template was resolved before allocating panes");
        let generated_labels = std::iter::once(active_pane_id)
            .chain(new_panes.iter().map(|(pane_id, _)| *pane_id))
            .zip(leaves.iter().map(|leaf| leaf.label.clone()))
            .collect::<Vec<_>>();
        debug_assert_eq!(generated_labels.len(), new_pane_count + 1);

        let replaced_stack_ids = replacing_active
            .then(|| {
                self.tabs[self.active_tab].pane(active_pane_id).map(|pane| {
                    pane.stack
                        .entries
                        .iter()
                        .map(|entry| entry.id)
                        .collect::<Vec<_>>()
                })
            })
            .flatten()
            .unwrap_or_default();
        for stack_id in replaced_stack_ids {
            self.background_observed_panes.remove(&stack_id);
        }

        let Some(stacked_launches) = self.install_pane_template_panes(
            PaneTemplateInstall {
                replacement,
                generated_labels,
                leaves: &leaves,
                new_panes: &new_panes,
                active_pane_id,
                replacing_active,
                active_wsl_cwd_file: active_wsl_cwd_file.clone(),
                working_directories: &working_directories,
            },
            cx,
        ) else {
            return false;
        };
        self.spawn_pane_template_terminals(
            PaneTemplateSpawns {
                tab_id,
                active_pane_id,
                replacing_active,
                leaves: &leaves,
                terminal_themes: &terminal_themes,
                working_directories: &working_directories,
                new_panes,
                active_wsl_cwd_file,
                stacked_launches,
                terminal_settings,
            },
            window,
            cx,
        );
        self.focus_active(window, cx);
        cx.notify();
        true
    }

    /// Each leaf's terminal theme, or the message to report on the window.
    ///
    /// Takes `&self` rather than `&mut self` because the caller is still holding
    /// a borrow of the active tab when it needs this.
    fn resolve_pane_template_themes(
        &self,
        leaves: &[ResolvedPaneSplitLeaf],
        active_pane_theme_override: Option<&str>,
        tab_theme_override: Option<&str>,
        project: Option<&ProjectConfig>,
        cx: &App,
    ) -> Result<Vec<Option<Arc<Theme>>>> {
        let themes = match leaves
            .iter()
            .enumerate()
            .map(|(index, leaf)| {
                resolve_terminal_theme(
                    (index == 0).then_some(active_pane_theme_override).flatten(),
                    tab_theme_override,
                    &leaf.profile,
                    project,
                    cx,
                )
            })
            .collect::<Result<Vec<_>>>()
        {
            Ok(themes) => themes,
            Err(error) => {
                return Err(error.context("Could not apply profile theme for pane template"));
            }
        };
        Ok(themes)
    }

    /// Reshapes the active tab into `replacement` and pushes the panes the
    /// template declared, returning the stacked commands still to launch.
    ///
    /// Returns `None` if the layout could not be replaced, which leaves the tab
    /// as it was rather than half-applied.
    fn install_pane_template_panes(
        &mut self,
        install: PaneTemplateInstall<'_>,
        cx: &mut Context<Self>,
    ) -> Option<Vec<(usize, u64, u64, String)>> {
        let PaneTemplateInstall {
            replacement,
            generated_labels,
            leaves,
            new_panes,
            active_pane_id,
            replacing_active,
            active_wsl_cwd_file,
            working_directories,
        } = install;
        let _ = cx;
        let active_leaf = &leaves[0];
        let new_pane_count = new_panes.len();
        let replaced_stack_ids = replacing_active
            .then(|| {
                self.tabs[self.active_tab].pane(active_pane_id).map(|pane| {
                    pane.stack
                        .entries
                        .iter()
                        .map(|entry| entry.id)
                        .collect::<Vec<_>>()
                })
            })
            .flatten()
            .unwrap_or_default();
        for stack_id in replaced_stack_ids {
            self.background_observed_panes.remove(&stack_id);
        }

        let tab = &mut self.tabs[self.active_tab];
        tab.maximized_pane = None;
        if !tab.layout.replace(active_pane_id, replacement) {
            return None;
        }
        if replacing_active {
            let pane = tab
                .pane_mut(active_pane_id)
                .expect("the active pane must remain in a template replacement");
            let _old_terminal = pane.terminal.take();
            pane.view = None;
            pane.error = None;
            pane.exit = None;
            pane.base_exited = false;
            pane.pending_command = None;
            pane.stack = PaneStack::default();
            pane.profile = active_leaf.profile.clone();
            pane.environment_overrides = active_leaf.environment.clone();
            pane.wsl_cwd_file = active_wsl_cwd_file.clone();
            apply_pane_split_overlay(pane, active_leaf);
        } else if let Some(pane) = tab.pane_mut(active_pane_id) {
            pane.profile = active_leaf.profile.clone();
            pane.environment_overrides = active_leaf.environment.clone();
            apply_pane_split_overlay(pane, active_leaf);
        }
        tab.panes.reserve(new_pane_count);
        for (index, (pane_id, wsl_cwd_file)) in new_panes.iter().enumerate() {
            let leaf = &leaves[index + 1];
            let mut pane = TerminalPane::new(*pane_id, leaf.profile.clone())
                .with_wsl_cwd_file(wsl_cwd_file.clone())
                .with_environment_overrides(leaf.environment.clone());
            apply_pane_split_overlay(&mut pane, leaf);
            tab.push_pane(pane);
        }
        tab.apply_generated_labels(generated_labels);
        tab.activate_pane(active_pane_id);
        self.retain_open_visible_terminals();

        // Every declared stacked entry is pushed before any spawn callback can
        // run, so the base terminal's spawn sees a non-base selection and leaves
        // focus to the stacked entry the pane ends up selecting.
        let stacked_leaves = std::iter::once(active_pane_id)
            .chain(new_panes.iter().map(|(pane_id, _)| *pane_id))
            .enumerate()
            .filter(|(leaf_index, _)| !leaves[*leaf_index].stack.is_empty());
        let mut stacked_launches = Vec::new();
        for (leaf_index, pane_id) in stacked_leaves {
            let leaf = &leaves[leaf_index];
            for command in &leaf.stack {
                let entry_id = self.next_pane_id;
                self.next_pane_id += 1;
                let entry = StackedPane::new(
                    entry_id,
                    command.clone(),
                    leaf.profile.clone(),
                    working_directories[leaf_index].0.clone(),
                    working_directories[leaf_index].1.clone(),
                );
                let pushed = self.tabs[self.active_tab]
                    .pane_mut(pane_id)
                    .is_some_and(|pane| pane.stack.push(entry));
                if !pushed {
                    break;
                }
                stacked_launches.push((leaf_index, pane_id, entry_id, command.clone()));
            }
        }
        Some(stacked_launches)
    }

    /// Starts a terminal in every pane the template declared, and every stacked
    /// command inside them.
    fn spawn_pane_template_terminals(
        &mut self,
        spawns: PaneTemplateSpawns<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let PaneTemplateSpawns {
            tab_id,
            active_pane_id,
            replacing_active,
            leaves,
            terminal_themes,
            working_directories,
            new_panes,
            active_wsl_cwd_file,
            stacked_launches,
            mut terminal_settings,
        } = spawns;
        let active_leaf = &leaves[0];
        let spawn_count = new_panes.len() + usize::from(replacing_active) + stacked_launches.len();
        if replacing_active {
            let path_hyperlink_regexes = terminal_settings.path_hyperlink_regexes(spawn_count == 1);
            self.spawn_terminal(
                TerminalSpawnRequest {
                    working_directory: working_directories[0].0.clone(),
                    wsl_directory: working_directories[0].1.clone(),
                    wsl_cwd_file: active_wsl_cwd_file,
                    terminal_theme: terminal_themes[0].clone(),
                    path_hyperlink_regexes,
                    environment: active_leaf.environment.clone(),
                    ..TerminalSpawnRequest::new(tab_id, active_pane_id, active_leaf.profile.clone())
                },
                &terminal_settings,
                window,
                cx,
            );
        }
        for (index, (pane_id, wsl_cwd_file)) in new_panes.into_iter().enumerate() {
            let leaf_index = index + 1;
            let path_hyperlink_regexes = terminal_settings
                .path_hyperlink_regexes(index + 1 + usize::from(replacing_active) == spawn_count);
            self.spawn_terminal(
                TerminalSpawnRequest {
                    working_directory: working_directories[leaf_index].0.clone(),
                    wsl_directory: working_directories[leaf_index].1.clone(),
                    wsl_cwd_file,
                    terminal_theme: terminal_themes[leaf_index].clone(),
                    path_hyperlink_regexes,
                    environment: leaves[leaf_index].environment.clone(),
                    ..TerminalSpawnRequest::new(tab_id, pane_id, leaves[leaf_index].profile.clone())
                },
                &terminal_settings,
                window,
                cx,
            );
        }
        let stacked_count = stacked_launches.len();
        for (index, (leaf_index, pane_id, entry_id, command)) in
            stacked_launches.into_iter().enumerate()
        {
            self.spawn_stacked_terminal(
                StackedTerminalSpawnRequest {
                    tab_id,
                    pane_id,
                    entry_id,
                    command,
                    profile: leaves[leaf_index].profile.clone(),
                    working_directory: working_directories[leaf_index].0.clone(),
                    wsl_directory: working_directories[leaf_index].1.clone(),
                    terminal_theme: terminal_themes[leaf_index].clone(),
                },
                &mut terminal_settings,
                index + 1 == stacked_count,
                window,
                cx,
            );
        }
    }

    fn replace_active_pane_profile(
        &mut self,
        profile: Profile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return false;
        };
        let tab_id = tab.id;
        let tab_theme_override = tab.theme_override.clone();
        let pane_theme_override = tab
            .pane(tab.active_pane)
            .and_then(|pane| pane.theme_override.clone());
        let active_pane_id = tab.active_pane;
        let active_pane = tab.active_pane();
        let effective_config = self.effective_config();
        let inherit_working_directory = effective_config
            .working_directory_scope
            .inherits_for_new_pane();
        let working_directory_configured = effective_config.working_directory_configured;
        let inherited_working_directory = active_pane
            .filter(|_| inherit_working_directory)
            .filter(|pane| !is_wsl_shell(&pane.profile.command))
            .and_then(|pane| pane.working_directory(cx));
        let inherited_wsl_directory = active_pane
            .filter(|_| inherit_working_directory)
            .and_then(|pane| pane.wsl_working_directory(cx));
        let (working_directory, wsl_directory) = launch_working_directory(
            &profile,
            inherited_working_directory,
            inherited_wsl_directory,
            self.working_directory.clone(),
            working_directory_configured,
        );
        let terminal_theme = match resolve_terminal_theme(
            pane_theme_override.as_deref(),
            tab_theme_override.as_deref(),
            &profile,
            self.active_project_config().map(AsRef::as_ref),
            cx,
        ) {
            Ok(theme) => theme,
            Err(error) => {
                self.configuration_error = Some(format!(
                    "Could not apply profile theme for pane replacement: {error:#}"
                ));
                cx.notify();
                return false;
            }
        };
        let mut terminal_settings = TerminalSpawnSettings::current(cx);
        let path_hyperlink_regexes = terminal_settings.path_hyperlink_regexes(true);
        let wsl_cwd_file = wsl_cwd_tracking_file(&profile, active_pane_id);

        let replaced_stack_ids = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.pane(active_pane_id))
            .map(|pane| {
                pane.stack
                    .entries
                    .iter()
                    .map(|entry| entry.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for stack_id in replaced_stack_ids {
            self.background_observed_panes.remove(&stack_id);
        }

        let Some(pane) = self.tabs[self.active_tab].pane_mut(active_pane_id) else {
            return false;
        };
        let _old_terminal = pane.terminal.take();
        pane.view = None;
        pane.error = None;
        pane.exit = None;
        pane.base_exited = false;
        pane.pending_command = None;
        pane.stack = PaneStack::default();
        pane.profile = profile.clone();
        pane.environment_overrides.clear();
        pane.wsl_cwd_file = wsl_cwd_file.clone();
        self.retain_open_visible_terminals();
        self.spawn_terminal(
            TerminalSpawnRequest {
                working_directory,
                wsl_directory,
                wsl_cwd_file,
                terminal_theme,
                path_hyperlink_regexes,
                ..TerminalSpawnRequest::new(tab_id, active_pane_id, profile)
            },
            &terminal_settings,
            window,
            cx,
        );
        self.focus_active(window, cx);
        cx.notify();
        true
    }
}

#[cfg(test)]
#[path = "../tests/app/pane_templates.rs"]
mod tests;

/// What installing a pane template into the active tab needs: the layout it
/// becomes, the panes that were allocated for it, and the labels the template
/// declared.
struct PaneTemplateInstall<'a> {
    replacement: PaneLayout,
    generated_labels: Vec<(u64, Option<String>)>,
    leaves: &'a [ResolvedPaneSplitLeaf],
    new_panes: &'a [(u64, Option<PathBuf>)],
    active_pane_id: u64,
    /// Whether the template's first leaf replaces the active pane rather than
    /// leaving it as it is.
    replacing_active: bool,
    active_wsl_cwd_file: Option<PathBuf>,
    working_directories: &'a [(Option<PathBuf>, Option<String>)],
}

/// What spawning a pane template's terminals needs, once the panes exist.
struct PaneTemplateSpawns<'a> {
    tab_id: u64,
    active_pane_id: u64,
    replacing_active: bool,
    leaves: &'a [ResolvedPaneSplitLeaf],
    terminal_themes: &'a [Option<Arc<Theme>>],
    working_directories: &'a [(Option<PathBuf>, Option<String>)],
    new_panes: Vec<(u64, Option<PathBuf>)>,
    active_wsl_cwd_file: Option<PathBuf>,
    stacked_launches: Vec<(usize, u64, u64, String)>,
    terminal_settings: TerminalSpawnSettings,
}
