//! Reloading configuration: the Reload action, its feedback, and what each
//! window does with a configuration a reload has prepared.
//!
//! The reload itself is split by thread. [`coordinator`] runs one at a time
//! and commits it in a fixed order; [`preparation`] is the half that reads the
//! disk and talks to the multiplexer, on a worker. This module is the
//! per-window part of the commit.

use super::*;

mod coordinator;
mod preparation;

#[cfg(feature = "zmux")]
use coordinator::run_on_worker;
pub(crate) use coordinator::{ReloadFailure, ReloadScope, request_configuration_reload};
use preparation::PreparedConfiguration;

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
        self.reload_configuration_then(window, cx, |_, _, _| {});
    }

    /// Reloads this window's configuration and calls `then` once the result
    /// has been committed, whether or not it succeeded.
    ///
    /// The reading happens on a worker, so the window keeps its current
    /// configuration — and keeps drawing — until the new one is ready.
    pub(crate) fn reload_configuration_then(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        self.configuration_reload_feedback.begin_attempt();
        cx.notify();
        let zetta = cx.entity().downgrade();
        let window_handle = window.window_handle();
        request_configuration_reload(
            ReloadScope::Windows(vec![zetta.clone()]),
            Box::new(move |outcome, cx| {
                let Some(zetta) = zetta.upgrade() else {
                    return;
                };
                let succeeded = outcome.window_result(zetta.entity_id()).is_ok();
                window_handle
                    .update(cx, |_, window, cx| {
                        zetta.update(cx, |this, cx| {
                            this.finish_configuration_reload(succeeded, window, cx);
                            then(this, window, cx);
                        });
                    })
                    .ok();
            }),
            cx,
        );
    }

    /// The success half of the Reload action's feedback. A failure has already
    /// been reported in `configuration_error` by the commit.
    fn finish_configuration_reload(
        &mut self,
        succeeded: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if succeeded {
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
        }
        cx.notify();
    }

    /// Phase 1 of a reload: what the worker needs from this window.
    ///
    /// Mux recovery is stopped here rather than at the commit: it reconfigures
    /// the same daemon, and would otherwise race the reload's own request.
    pub(crate) fn configuration_reload_snapshot(&mut self) -> WindowReloadSnapshot {
        #[cfg(feature = "session-persistence")]
        self.invalidate_mux_recovery();
        WindowReloadSnapshot {
            project_roots: self.reloadable_project_roots(),
            #[cfg(feature = "zmux")]
            mux: self.mux.as_ref().map(MuxRuntime::reconfigure_handle),
        }
    }

    /// The registered project roots this window has loaded or has a pane in.
    fn reloadable_project_roots(&self) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        for root in self
            .projects
            .configs
            .keys()
            .chain(self.projects.pane_roots.values())
            .filter(|root| self.projects.registry.contains(root))
        {
            if !roots.contains(root) {
                roots.push(root.clone());
            }
        }
        roots
    }

    /// Phase 3 of a reload, for one window: adopts a configuration the
    /// coordinator has already validated for it.
    ///
    /// Everything this reads was prepared on the worker; nothing here touches
    /// the disk or the daemon. What the window changed while that ran is not
    /// covered by the preparation, and is caught up rather than committed
    /// stale: a project root it gained, or a multiplexer connection it opened
    /// with the old policy. Returns an error only for a part that could not be
    /// applied; everything else is committed regardless.
    pub(crate) fn commit_prepared_configuration(
        &mut self,
        prepared: &PreparedConfiguration,
        snapshot: &WindowReloadSnapshot,
        projects: Vec<Arc<ProjectConfig>>,
        cx: &mut Context<Self>,
    ) -> std::result::Result<(), String> {
        let config = prepared.config.as_ref();
        // A configuration reload is the boundary at which session-scoped pane
        // and tab theme selections are intentionally discarded. Clear both
        // visible and process-local detached tabs, and advance the generation
        // used to reject older live multiplexer state from this same process.
        self.configuration_generation = self.configuration_generation.wrapping_add(1);
        self.discard_session_theme_overrides();
        self.refresh_pane_profiles(config);
        self.profile_shortcut_slots =
            visible_profile_count(&config.profiles, &config.hidden_profiles);

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
        self.launch_config = config.clone();
        self.configuration_error = None;
        let settings_error = self.settings_editor.as_mut().and_then(|editor| {
            match &prepared.settings_form {
                Ok(form) => {
                    editor.refresh_configuration(&self.launch_config, form);
                    None
                }
                // A dirty form is never replaced, so it cannot fail to be.
                Err(error) => (!editor.configuration_dirty).then(|| error.clone()),
            }
        });
        #[cfg(feature = "session-persistence")]
        self.install_prepared_auto_protect(&prepared.auto_protect, cx);
        #[cfg(feature = "session-persistence")]
        self.start_mux_recovery_if_needed(cx);
        self.project_detection_base = prepared.config.clone();
        self.install_reloaded_project_configs(&snapshot.project_roots, projects, cx);
        self.command_palette = None;
        // The process-wide keymap was bound for the user configuration's
        // profiles; an active project can resolve to a different set, and this
        // also rebuilds the native macOS menus for it.
        self.refresh_effective_project_context(Some(&prepared.keymap), cx);
        #[cfg(feature = "zmux")]
        self.reconfigure_replaced_mux_runtime(snapshot, cx);
        if let Some(error) = settings_error.as_ref() {
            self.configuration_error = Some(
                ReloadFailure::Apply(error.clone()).window_message(&self.launch_config.config_path),
            );
        }
        cx.notify();
        settings_error.map_or(Ok(()), Err)
    }

    fn discard_session_theme_overrides(&mut self) {
        for tab in self
            .tabs
            .iter_mut()
            .chain(self.background_sessions.iter_mut())
        {
            tab.theme_override = None;
            for pane in &mut tab.panes {
                pane.theme_override = None;
                for entry in &mut pane.stack.entries {
                    entry.theme_override = None;
                }
            }
        }
    }

    /// Points every pane, and every pane stacked under one, at the reloaded
    /// definition of its profile. A profile that no longer exists keeps the
    /// pane's command but loses its configured themes.
    fn refresh_pane_profiles(&mut self, config: &Config) {
        let launch_theme_override = self.launch_theme_override.as_ref();
        let refresh = |profile: &mut Profile| {
            if let Some(reloaded) = config
                .profiles
                .iter()
                .find(|reloaded| reloaded.name.eq_ignore_ascii_case(&profile.name))
            {
                *profile = reloaded.clone();
                crate::app::apply_launch_theme_override(profile, launch_theme_override);
            } else {
                profile.theme = None;
                profile.dark_theme = None;
            }
        };
        for pane in self.tabs.iter_mut().flat_map(|tab| &mut tab.panes) {
            refresh(&mut pane.profile);
            for entry in &mut pane.stack.entries {
                refresh(&mut entry.profile);
            }
        }
    }

    #[cfg(feature = "session-persistence")]
    fn install_prepared_auto_protect(
        &mut self,
        prepared: &std::result::Result<
            Option<Arc<crate::session_auto_protect::SessionAutoProtect>>,
            String,
        >,
        cx: &mut Context<Self>,
    ) {
        // Rejects whatever a `refresh_auto_protect` still in flight resolves
        // from the configuration this replaces.
        self.auto_protect_generation = self.auto_protect_generation.wrapping_add(1);
        match prepared {
            Ok(auto_protect) => self.auto_protect = auto_protect.clone(),
            Err(error) => {
                self.auto_protect = None;
                let message = format!("Could not set up automatic session protection: {error}");
                // Where `refresh_auto_protect` has always reported each kind.
                if crate::session_auto_protect::SessionAutoProtect::resolution_is_blocking(
                    &self.launch_config.sessions.persistence,
                ) {
                    self.show_error_notice(message, cx);
                } else {
                    self.configuration_error = Some(message);
                }
            }
        }
    }

    /// Replaces the window's project configurations with the reloaded ones.
    ///
    /// A root the window gained while the reload was being prepared was loaded,
    /// if at all, against the configuration this replaces; it is reloaded
    /// rather than kept.
    fn install_reloaded_project_configs(
        &mut self,
        prepared_roots: &[PathBuf],
        projects: Vec<Arc<ProjectConfig>>,
        cx: &mut Context<Self>,
    ) {
        let late_roots = self
            .reloadable_project_roots()
            .into_iter()
            .filter(|root| !prepared_roots.contains(root))
            .collect::<Vec<_>>();
        self.projects.configs.clear();
        for project in projects {
            self.projects.insert_shared_config(project);
        }
        self.reload_stale_project_configs(late_roots, cx);
    }

    /// Loads `roots` against the current configuration, off the GUI thread,
    /// for project configurations that were loaded against an older one.
    ///
    /// Until they land the window resolves those panes as outside any project,
    /// which is the safe direction: a stale project would run commands with a
    /// profile set that no longer exists.
    pub(crate) fn reload_stale_project_configs(
        &mut self,
        roots: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if roots.is_empty() {
            return;
        }
        let base = self.project_detection_base.clone();
        let generation = self.configuration_generation;
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    roots
                        .into_iter()
                        .map(|root| {
                            let result = ProjectConfig::load(&root, &base);
                            (root, result)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                // A newer reload snapshotted these roots itself.
                if this.configuration_generation != generation {
                    return;
                }
                for (root, result) in loaded {
                    match result {
                        Ok(project) => {
                            this.projects.insert_config(project);
                        }
                        Err(error) => {
                            this.projects.configs.remove(&root);
                            this.configuration_error = Some(format!(
                                "Could not load project configuration {}: {error:#}",
                                ProjectConfig::path_for(&root).display()
                            ));
                        }
                    }
                }
                this.refresh_effective_project_context(None, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Re-derives everything that follows from the active project: the tab
    /// icon, each tab's themes, the effective profiles and working directory,
    /// and the profile shortcuts.
    fn refresh_effective_project_context(
        &mut self,
        keymap: Option<&KeymapSource>,
        cx: &mut Context<Self>,
    ) {
        self.projects.invalidate_active_context();
        let active_project = self.active_project_config().cloned();
        self.refresh_active_project_tab_icon(active_project.as_deref());
        let tab_ids = self.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
        for tab_id in tab_ids {
            self.apply_effective_themes_to_tab(tab_id, cx);
        }
        let (effective_profiles, effective_working_directory) = {
            let effective = self
                .active_project_config()
                .map_or(&self.launch_config, |project| &project.effective);
            (
                effective.profiles.clone(),
                effective.working_directory.clone(),
            )
        };
        self.profiles = effective_profiles;
        self.working_directory = effective_working_directory;
        self.refresh_profile_shortcuts_from(keymap, cx);
        cx.notify();
    }

    /// Sends the reloaded policy to a multiplexer connection the window opened
    /// while the reload was being prepared. That connection was configured from
    /// the configuration this replaced, and the preparation never saw it.
    #[cfg(feature = "zmux")]
    fn reconfigure_replaced_mux_runtime(
        &mut self,
        snapshot: &WindowReloadSnapshot,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.mux.as_ref().map(MuxRuntime::reconfigure_handle) else {
            return;
        };
        if snapshot.daemon_identity() == Some(handle.identity()) {
            return;
        }
        let sessions = self.launch_config.sessions.clone();
        let generation = self.configuration_generation;
        let reconfigured = run_on_worker("zetta-mux-reconfigure", move || {
            crate::mux::MuxReconfiguration::resolve(&sessions)
                .and_then(|plan| handle.reconfigure(&plan))
        });
        cx.spawn(async move |this, cx| {
            let result = reconfigured
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("the reconfigure worker stopped")));
            this.update(cx, |this, cx| {
                if this.configuration_generation != generation {
                    return;
                }
                match result {
                    Ok(()) => {
                        #[cfg(feature = "session-persistence")]
                        this.start_mux_recovery_if_needed(cx);
                    }
                    Err(error) => {
                        this.configuration_error = Some(
                            ReloadFailure::Apply(format!("{error:#}"))
                                .window_message(&this.launch_config.config_path),
                        );
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }
}

/// What a running reload recorded about one window when it started.
pub(crate) struct WindowReloadSnapshot {
    pub(crate) project_roots: Vec<PathBuf>,
    #[cfg(feature = "zmux")]
    pub(crate) mux: Option<crate::mux::MuxReconfigureHandle>,
}

impl WindowReloadSnapshot {
    pub(crate) fn daemon_identity(&self) -> Option<usize> {
        #[cfg(feature = "zmux")]
        return self
            .mux
            .as_ref()
            .map(crate::mux::MuxReconfigureHandle::identity);
        #[cfg(not(feature = "zmux"))]
        None
    }
}

#[cfg(test)]
#[path = "tests/configuration_reload.rs"]
mod tests;
