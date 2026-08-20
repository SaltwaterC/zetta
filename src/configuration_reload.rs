use super::*;

pub(crate) const CONFIGURATION_RELOAD_SUCCESS_MESSAGE: &str = "Configuration reloaded";
const CONFIGURATION_RELOAD_SUCCESS_DURATION: Duration = Duration::from_secs(3);

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ConfigurationReloadFeedback {
    visible: bool,
    generation: u64,
}

impl ConfigurationReloadFeedback {
    fn begin_attempt(&mut self) {
        self.visible = false;
        self.generation = self.generation.wrapping_add(1);
    }

    fn show_success(&mut self) -> u64 {
        self.visible = true;
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    fn dismiss_if_current(&mut self, generation: u64) -> bool {
        if self.generation != generation || !self.visible {
            return false;
        }
        self.visible = false;
        true
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }
}

impl Zetta {
    pub(crate) fn edit_config_file(
        &mut self,
        _: &EditConfigFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = self.launch_config.config_path.clone();
        self.edit_settings_file_in_active_pane(path, window, cx);
    }

    pub(crate) fn edit_keymap_file(
        &mut self,
        _: &EditKeymapFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = self.launch_config.keymap_path.clone();
        self.edit_settings_file_in_active_pane(path, window, cx);
    }

    /// Runs Zetta's editor dispatcher against the active pane's shell, mirroring how a
    /// clicked path or `EditScrollback` opens an editor: reused in place when the pane's
    /// foreground process is the shell, otherwise split into a fresh pane.
    pub(crate) fn edit_settings_file_in_active_pane(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let tab_id = tab.id;
        let Some(pane) = tab.active_pane() else {
            return;
        };
        let pane_id = pane.id;
        let Some(terminal) = pane.terminal.clone() else {
            return;
        };
        let (command, open_in_new_pane) = terminal.update(cx, |terminal, _| {
            (
                terminal.editor_command_for_path(&path, terminal.native_path_style()),
                terminal.editor_should_open_in_new_pane(),
            )
        });
        let Some(command) = command else {
            return;
        };
        if open_in_new_pane {
            self.open_editor_in_new_pane(
                tab_id,
                pane_id,
                terminal_view::EditorRequest {
                    command,
                    temporary_path: None,
                },
                window,
                cx,
            );
        } else {
            terminal.update(cx, |terminal, _| terminal.submit_editor_command(command));
        }
    }

    pub(crate) fn reload_configuration(
        &mut self,
        _: &ReloadConfiguration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.configuration_reload_feedback.begin_attempt();
        let config_path = self.launch_config.config_path.clone();
        let keymap_override = self.launch_config.keymap_override.clone();
        let config = match Config::load(Some(&config_path), keymap_override) {
            Ok(config) => config,
            Err(error) => {
                self.configuration_error = Some(format!(
                    "Could not load {}: {error:#}",
                    config_path.display()
                ));
                cx.notify();
                return;
            }
        };

        if let Err(error) = self.apply_loaded_configuration(config, cx) {
            self.configuration_error = Some(format!(
                "Could not apply {}: {error:#}",
                config_path.display()
            ));
            cx.notify();
            return;
        }
        self.configuration_error = None;
        let generation = self.configuration_reload_feedback.show_success();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            executor.timer(CONFIGURATION_RELOAD_SUCCESS_DURATION).await;
            this.update(cx, |this, cx| {
                if this
                    .configuration_reload_feedback
                    .dismiss_if_current(generation)
                {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn reload_configuration_from_process(
        &mut self,
        config: Config,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        self.apply_loaded_configuration(config, cx)?;
        self.configuration_error = None;
        Ok(())
    }

    fn apply_loaded_configuration(&mut self, config: Config, cx: &mut Context<Self>) -> Result<()> {
        load_user_themes(cx).log_err();
        let project_detection_base = Arc::new(config.clone());
        let project_roots = self
            .projects
            .configs
            .keys()
            .chain(self.projects.pane_roots.values())
            .filter(|root| self.projects.registry.contains(root))
            .cloned()
            .collect::<HashSet<_>>();
        let project_configs = project_roots
            .iter()
            .map(|root| {
                ProjectConfig::load(root, &config).with_context(|| {
                    format!(
                        "reloading project configuration {}",
                        ProjectConfig::path_for(root).display()
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?;
        apply_config_settings(&config, cx)?;
        let profile_themes = config
            .profiles
            .iter()
            .map(|profile| {
                resolve_profile_theme(profile, cx).map(|theme| (profile.name.to_lowercase(), theme))
            })
            .collect::<Result<HashMap<_, _>>>()?;
        for pane in self.tabs.iter_mut().flat_map(|tab| &mut tab.panes) {
            if let Some(profile) = config
                .profiles
                .iter()
                .find(|profile| profile.name.eq_ignore_ascii_case(&pane.profile.name))
            {
                pane.profile = profile.clone();
            } else {
                pane.profile.theme = None;
            }
            for entry in &mut pane.stack.entries {
                if let Some(profile) = config
                    .profiles
                    .iter()
                    .find(|profile| profile.name.eq_ignore_ascii_case(&entry.profile.name))
                {
                    entry.profile = profile.clone();
                } else {
                    entry.profile.theme = None;
                }
            }
            let theme = profile_themes
                .get(&pane.profile.name.to_lowercase())
                .cloned()
                .flatten();
            for view in pane.all_views().cloned().collect::<Vec<_>>() {
                view.update(cx, |view, cx| view.set_theme(theme.clone(), cx));
            }
        }
        let profile_count = visible_profile_count(&config.profiles, &config.hidden_profiles);
        load_keybindings(&config.keymap_path, profile_count, cx);
        self.profile_shortcut_slots = profile_count;

        #[cfg(windows)]
        windows_integration::update_profile_jump_list(config.profiles.clone());

        if config.pane_controls_hidden_by_default
            != self.launch_config.pane_controls_hidden_by_default
        {
            reset_pane_controls_visibility(
                &mut self.pane_controls_hidden_for,
                config.pane_controls_hidden_by_default,
                self.tabs
                    .iter()
                    .flat_map(|tab| tab.panes.iter().map(|pane| pane.id)),
            );
            self.pane_controls_visible_for = None;
        }
        self.profiles = config.profiles.clone();
        self.working_directory = config.working_directory.clone();
        self.launch_config = config;
        self.project_detection_base = project_detection_base;
        self.projects.configs.clear();
        for project in project_configs {
            self.projects.insert_config(project);
        }
        self.projects.invalidate_active_context();
        let active_project = self.active_project_config().cloned();
        self.refresh_active_project_tab_icon(active_project.as_deref());
        let project_tab_ids = self
            .tabs
            .iter()
            .filter(|tab| self.projects.config_for_pane(tab.active_pane).is_some())
            .map(|tab| tab.id)
            .collect::<Vec<_>>();
        for tab_id in project_tab_ids {
            self.apply_effective_themes_to_tab(tab_id, cx);
        }
        let (effective_profiles, effective_working_directory) = {
            let effective = self
                .active_project_config()
                .map(|project| &project.effective)
                .unwrap_or(&self.launch_config);
            (
                effective.profiles.clone(),
                effective.working_directory.clone(),
            )
        };
        self.profiles = effective_profiles;
        self.working_directory = effective_working_directory;
        self.command_palette = None;
        // The reload above bound one shortcut per visible profile of the user
        // configuration; an active project can resolve to a different set, and
        // this also rebuilds the native macOS menus for it.
        self.refresh_profile_shortcuts(cx);
        cx.notify();
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/configuration_reload.rs"]
mod tests;
